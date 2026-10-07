# Development and validation

macOS builds require macOS 14.0 or later.

## macOS prerequisites

GPUI compiles Metal shaders as part of the macOS build. Install Xcode and make
sure the selected Xcode has Apple's optional Metal Toolchain. Recent Xcode
installations may not include it initially:

```sh
xcodebuild -downloadComponent MetalToolchain
```

If multiple Xcode versions are installed, select the intended version with
`xcode-select` before downloading the component. `mise run doctor` checks that
Xcode is selected, its Swift compiler runs, and `xcrun` can locate the Metal
compiler. Apple documents both the Xcode settings and command-line installation
paths in
[Downloading and installing additional Xcode components][apple-components].

[apple-components]: https://developer.apple.com/documentation/xcode/downloading-and-installing-additional-xcode-components

The AppKit smoke helper uses Xcode's bundled Swift compiler; no separate Swift
installation is needed. `mise run build:swift` compiles it independently of Rust
and Ghostty. Both the native smoke tasks and CodeQL use this task.

CodeQL's advanced workflows scan Actions, JavaScript/TypeScript, Rust, and Swift
with the security-extended query suite. Swift uses a manual build because the
helper is a standalone file, not an Xcode project or Swift package. Swift lives
in `.github/workflows/codeql-swift.yml` and runs only when Swift sources or that
workflow change, on the weekly schedule, or by manual dispatch: its macOS runner
competes with CI's macOS jobs for the account's macOS concurrency limit.
`codeql.yml` skips Release Please pull requests, which change only versions and
the changelog. When enabling these workflows, switch the repository from default
to advanced CodeQL setup: default setup blocks uploads from custom CodeQL
workflows. Verify all four language jobs and their uploaded analyses before
retiring the old default-setup analysis configurations.

## Ubuntu 22.04 prerequisites

Install GPUI's X11 link libraries and the software Vulkan driver used by the
headless smoke test:

```sh
sudo apt-get install --no-install-recommends \
  libxkbcommon-dev libxkbcommon-x11-dev mesa-vulkan-drivers xvfb \
  xdotool x11-xkb-utils x11-utils openbox xcompmgr xclip tmux
```

These packages are tracked by apt and can be removed with `sudo apt-get remove`
if they are not needed by another application. Other Linux distributions need
the equivalent XKB development libraries, Vulkan driver, and Xvfb packages.
Run `mise run doctor` to check the link-time prerequisites.

Then install the pinned Rust and validation tools plus the local hook:

```sh
mise run setup
```

## Linked worktrees

`treeboot.toml` bootstraps a new linked worktree with `treeboot run`: it runs
`mise run setup`, then `mise run ghostty:prepare`. A warm machine needs no
network for it.

`ghostty:prepare` and `sparkle:prepare` look for their pinned archive in the
repository's other checkouts before downloading, the primary checkout first.
They list checkouts with `git worktree list`, copy a candidate into
`.native/*/archives`, and keep it only when its SHA-256 matches the manifest.
A checkout on another pin is skipped, so a stale primary checkout costs
nothing but does not help either. Only the archive is reused; each worktree
still extracts and verifies its own source tree.

## Linux checks through Docker

Docker can run the Linux checks from macOS or Linux. The runner defaults to
Docker's native architecture and accepts `--arch amd64` or `--arch arm64`.
Docker must already be running with a Linux engine; non-native architectures
require working emulation in that engine.

```sh
mise run linux:test
mise run linux:smoke
mise run package:linux:container
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
behavior. Keep timing benchmarks on native hardware, except
`bench:renderer-scenarios`: it times CPU work only, so before and after reports
from the same container are comparable. See
[the renderer performance notes](../performance/gpui-terminal-renderer.md).

`package:linux:container` builds and verifies the native-architecture AppImage
and tarball in the pinned Ubuntu 22.04 image, then replaces only the matching
`dist/linux/<architecture>` directory in the host checkout. Docker copies the
verified output from the stopped build container through the host client, so
the resulting artifacts are owned by the invoking user under rootful and
rootless engines. Pass
`-- --arch amd64` or `-- --arch arm64` to select an architecture; a non-native
selection requires working Docker emulation.

The first invocation builds a local Ubuntu 22.04 image with pinned Mise, Rust,
Bun, and Zig. The Ubuntu index digest and Mise archive checksums are in
`scripts/linux/Dockerfile`; tool versions come from `mise.toml`, `mise.lock`,
and `rust-toolchain.toml`. Apt packages resolve to Ubuntu's updates when the
image is built. Image reuse depends on those files and the container
entrypoint, so ordinary source edits do not rebuild the image. Images are
local and are not published.

The checkout is mounted read-only, then copied into a Docker volume before
each run. This includes current uncommitted and untracked source files and
removes obsolete copies. Host `.git`, `dist`, `target`, `.native`,
`node_modules`, and `.codegraph` directories are excluded. Package runs also
remove retained workspace `dist` output before building, so artifacts from an
older version cannot enter the new export. The copied workspace has no Git
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

## macOS smokes through Tart

On Apple Silicon, the macOS desktop smokes can run in a disposable
[Tart](https://tart.run) VM instead of on the host display. The VM keeps smoke
windows, focus changes, fullscreen Spaces, cursor warps, and global hotkeys off
the host, so parallel sessions do not collide. Install Tart first; these
tasks sit alongside the host `smoke:macos-*` tasks and do not replace them.

```sh
mise run vm:macos:smoke
mise run vm:macos:smoke -- macos-quake
mise run vm:macos:exec -- mise exec -- \
  bun scripts/check-palette.ts target/debug/examples/palette_smoke
mise run vm:macos:dev
mise run vm:macos:clean
```

`vm:macos:smoke` builds the CI smoke binaries on the host, then runs
`ci:smoke:run`, or the named `HUTERM_CI_SMOKE_STEP`, in a headless guest.
`vm:macos:exec` runs a command against whatever host outputs already exist and
does not build. Use `mise exec --` inside the guest command for managed tools
such as Bun; raw `exec` does not activate their paths. `vm:macos:dev` builds
`target/debug/huterm`, opens a Tart window, and streams the app's output to the
host terminal.

`dev` then stays in charge of that instance so an edit costs a rebuild instead
of another boot. Press `r` to rebuild and replace the running app, `w` to
toggle watching the source trees, and `q` to quit. A save while watching
rebuilds automatically, and edits arriving during a build collapse into one
follow-up cycle. A failed build leaves the running app alone. Quitting Huterm
inside the VM returns to the same prompt rather than ending the session, and
`q` stops the VM while keeping it, so anything installed or configured in it
survives. One dev session runs per worktree at a time. With a warm build the
cycle is about 6 seconds, against roughly 35 for a boot.

The first run pulls the digest-pinned Cirrus Labs macOS 27 base image (about
33 GB) and provisions a local `huterm-macos-<hash>` image with pinned Mise,
Bun from `mise.lock`, and tmux. The hash covers the base image,
`scripts/macos-vm/provision.sh`, and `mise.lock`. Every run clones that image
through APFS copy-on-write, shares the checkout read-only, and copies the
sources and host-built runtime outputs into the guest. Smokes and `exec` delete
their clone afterwards; `dev` keeps `huterm-macos-vm-<hash>` for next time. A
kept dev VM still uses the image it was cloned from, so remove it with
`vm:macos:clean` to pick up a newer one. Expect about 35 seconds of overhead
per run. The guest never compiles, so its macOS version must satisfy host-built
binaries: Swift witnesses built with Xcode 27 require macOS 27.

Virtualization.framework runs at most two macOS guests per host. The runner
queues for one of two slots, shared by every worktree, and waits up to 30
minutes. macOS VMs started outside these tasks, including other Tart, UTM, or
Parallels guests, also count toward the limit; Tart then fails with "The
number of VMs exceeds the system limit".

`vm:macos:clean` removes provisioned images, this worktree's kept dev VM, and
leftover run VMs once no run holds a slot. Other worktrees keep their own dev
VMs. The base image stays cached; remove it with
`tart delete <image>` using the reference printed by the task. The guest has
one 1280x800 display without a notch, and paravirtualized Metal, so keep
benchmarks, notch and safe-area checks, and multi-display QA on real hardware.

## Interactive Linux desktop through Tart

On Apple Silicon, `vm:linux:*` runs a Linux build in a GNOME desktop VM. Docker
stays the path for Linux tests and smokes; this VM is for manual and visual QA
that needs a real desktop session, a window manager, a file manager, or
Wayland. Install Tart first.

```sh
mise run vm:linux:dev
mise run vm:linux:dev:x11
mise run vm:linux:exec -- uname -a
mise run vm:linux:clean
```

`vm:linux:dev` builds `huterm` in the pinned Ubuntu 22.04 container, copies the
executable and terminfo out of the container's workspace volume, boots this
worktree's VM with a window, and runs the binary in the desktop session with its
output streamed to the host terminal. It then keeps the same `r`, `w` and `q`
controls as the macOS session, rebuilding through the container and replacing
the running instance in place; a warm rebuild cycle is a few seconds. `q` stops
the VM. The VM
itself is kept, so installed packages and files survive; a run with a warm
container build takes about 20 seconds end to end, of which the guest boots in
roughly 8. `vm:linux:dev` uses the GNOME Wayland session, where Huterm runs
through XWayland, and `vm:linux:dev:x11` uses GNOME on Xorg. Switching sessions
restarts GDM and takes about 25 seconds; running the same session again does
not. `vm:linux:exec` runs any command in that session and does not build.

The first run pulls the digest-pinned Ubuntu 24.04 base image (about 3 GB) and
provisions `huterm-linux-image-<hash>` with GNOME, GDM autologin, software
Vulkan, and fonts. The hash covers the base image and
`scripts/linux-vm/provision.sh`. Ubuntu 24.04 is deliberate: it is the last LTS
shipping both the Wayland and Xorg GNOME sessions. Expect about 7 GB for the
image plus each worktree's VM.

Cross-compiling from macOS is not supported. `huterm-gpui` links Linux system
libraries, and the container reproduces the glibc 2.35 ABI ceiling that CI and
packaging depend on. Tart guests are arm64 only, because
Virtualization.framework does not emulate; use the container's `--arch amd64`
emulation for x86_64 checks.

A dev session and an `exec` command can share one VM. Commands hold a shared
lock for their duration, and only the last one out stops the VM. Changing
sessions requires that no other command is running.

`vm:linux:clean` removes the provisioned image and this worktree's VM once no
command is active; other worktrees keep their own. The base image stays cached;
remove it with `tart delete` using the reference the task prints.

Product behavior, configuration, and default shortcuts are documented in the
[README](../../README.md); update it, not this guide, when they change.
Tab-bar layout mechanics are in the [desktop client guide](desktop-client.md),
and Quake scheduling is in the [window presentation guide](window-presentation.md).

## Rust test layout

Unit tests stay in the crate as child modules of the code they test, so they
keep private access. A small suite stays inline at the end of its module. A
suite that dominates its file moves to an adjacent file without changing its
module path:

```rust
// crates/huterm-gpui/src/scroll.rs
#[cfg(test)]
mod tests;
```

The body, without the `mod tests { ... }` wrapper, lives in
`crates/huterm-gpui/src/scroll/tests.rs` and still starts with
`use super::*;`. A `mod.rs` file puts it beside itself, as in
`desktop/palette/tests.rs`. Named suites keep their names, such as
`engine/clipboard_tests.rs` for `mod clipboard_tests;` in `engine.rs`. Keep
every attribute, including `#[cfg(...)]`, on the declaration in the parent.

When moving a suite:

- Move the body verbatim and let rustfmt remove the outer indentation. A
  textual dedent can change the contents of multi-line string literals.
  rustfmt leaves statements containing over-long literals at their old
  indentation, so fix those lines by hand. Its reflow can also shorten a test
  enough to leave a `#[expect(clippy::too_many_lines)]` unfulfilled; remove
  that expectation rather than adding `#[rustfmt::skip]`.
- Relative `include_str!` and `include_bytes!` paths gain one `../` for each
  added directory level.
- A file loaded through `#[path]` resolves child modules as if it were a
  `mod.rs`. Name the test file explicitly there, as
  `desktop/quake_windows/policy.rs` does with
  `#[path = "policy/tests.rs"]`.
- Compare the sorted output of
  `cargo test --workspace --all-targets --locked -- --list`, with and without
  `--ignored`, before and after the move. Then run each moved module through
  its path filter and check that the passed count matches the listed count.

`mise run lint:test-order` runs `scripts/rust-test-order.ts` from `check`,
`check:rust`, `ci:lint`, and the pre-commit hook. It rejects any item without
a test-only cfg (`test`, or an `all(...)` containing it) that follows an inline
test module in the same module. Out-of-line `mod tests;` declarations may sit
anywhere. Clippy's `items_after_test_module` covers only an inline module
named exactly `tests` with no later module.

## Validation ladder

| Trigger | Command | Scope | Evidence owner |
| --- | --- | --- | --- |
| Iteration | focused `cargo test -p <crate> <test>` | Changed behavior | Implementer |
| Pre-commit | Lefthook change-aware jobs | Staged files where checks work per file; affected script tests; whole-project analysis only for matching inputs | Local hook |
| Handoff | `mise run verify` | Check, tests, licenses, workflows | Implementer |
| Pull request | `mise run format:check`, `mise run ci:lint`, `mise run schema:check`, `mise run check:scripts`, and `mise run ci:test` on `macos-15`, Ubuntu 24.04 x86_64, and Ubuntu 24.04 aarch64 | Rust formatting, Clippy, test-module order, protocol boundaries, generated schemas, scripts, and Rust tests in one job per platform | CI |
| Pull request | Named platform smoke steps on `macos-15`, Ubuntu 24.04 x86_64, and Ubuntu 24.04 aarch64, supervised by `mise run ci:smoke:step` as slices of `mise run ci:smoke:run`; equivalent to the local `mise run ci:smoke` aggregate and order | Cached native source preparation, one smoke binary compilation, then serial desktop smoke execution for each platform | CI |
| Pull request | `mise run lint:docs`, `lint:agents`, `ci:workflows`, `smoke:schema-editor`, `vendor:check`, `license`, and `audit:scripts` as separate Policy steps on Ubuntu 24.04 | Documentation, agent guides, workflows, editor schema behavior, vendor, Cargo dependency, and scripting dependency policy | CI |
| Linux smoke | `mise run smoke:linux` | GPUI window remains live under Xvfb | CI or implementer |
| Linux keyboard | `mise run smoke:linux-input` | XTest input through XKB, shortcut dispatch, and a raw Ghostty PTY | CI or implementer |
| Linux clipboard | `mise run smoke:linux-clipboard` | Exact OSC 52 and tmux writes through Ghostty to an isolated X11 CLIPBOARD selection | CI or implementer |
| macOS clipboard | `mise run smoke:macos-clipboard` | Exact OSC 52 and tmux writes through Ghostty, including NUL, with pasteboard preservation | CI or implementer |
| Linux fullscreen | `mise run smoke:linux-fullscreen` | Openbox EWMH property, geometry, PTY input/resize, ignored-request timeout, and Quit capture | CI or implementer |
| macOS fullscreen | `mise run smoke:macos-fullscreen` | AppKit modes, style/focus restoration, retained tabs, presentation leases, and PTY input/resize | CI or implementer |
| Linux quake | `mise run smoke:linux-quake` | Native XTest shortcuts, external focus, composited fade pixels, animations, OS grab rollback, and PTY lifecycle | CI or implementer |
| macOS quake | `mise run smoke:macos-quake` | Native session shortcuts, external AppKit focus, alpha/geometry, Space exit, and PTY lifecycle; requires event-posting permission | CI or implementer |
| macOS menus | `mise run smoke:macos-menus` | Real AppKit shortcut values at startup and reload | CI or implementer |
| macOS keyboard | `mise run smoke:macos-input` | Native input and composition through Ghostty, plus one legacy-config launch | CI or implementer |
| macOS refresh | `mise run smoke:macos-refresh` | Native frame stop/resume, one pending callback across tab replacement, and idle activity-task cancellation; requires an unlocked GUI session | Implementer |
| macOS Quit | `mise run smoke:macos-quit` | Cancellable AppKit termination through Ghostty | CI or implementer |
| Scroll benchmark | `mise run ci:benchmarks` on Ubuntu 24.04 | Ghostty snapshot timing, offsets, and queue bounds; paint timing and row reuse when frames arrive | CI or implementer |
| macOS package | `mise run package:macos` | Universal app metadata, icon, executable, and both architectures | CI or implementer |
| Linux package | `mise run package:linux` | Native AppImage and relocatable tarball, dependency policy, provenance, payload equality, and Ghostty input smoke | CI or implementer |

CI groups format, static analysis, scripts, and Rust tests into one job per
platform so their setup and debug artifacts are reused. Native desktop smokes,
Linux release benchmarks, and macOS packaging remain separate because combining
them would lengthen the workflow's slowest path. Within the Checks and Policy
jobs, each validation step runs once setup succeeds even if an earlier check
failed, so one run reports every failing check under its own step name. The
final
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
on a hosted runner image. Because it omits Rust's environment hash, the key also
names `CARGO_PROFILE_DEV_DEBUG`; CI sets `line-tables-only` so debug builds and
caches stay smaller while backtraces keep symbols. Because native smoke
execution can fail transiently after compilation succeeds, main still saves its
smoke build cache on failure so the requested rerun does not compile from
scratch.

Only runs on main, including manual dispatches, save Rust build caches. Pull
requests restore main's entries; per-PR copies pushed the repository past
GitHub's 10 GB cache limit, and the resulting evictions made unrelated jobs
compile cold.

Each CI smoke step has a five-minute process deadline and a six-minute Actions
backstop. The supervisor streams output and records stdout, stderr, elapsed-time
events, and exit status under `HUTERM_SMOKE_EVIDENCE_DIR/steps/<step>`. Evidence
is uploaded for successful and failed jobs so timing can be compared. The
renderer also records its native process events under `renderer-process`,
distinguishing paint completion from process exit. Its 15-second deadline and
completion assertions remain enforced.

After compilation succeeds, independent smoke steps continue after a failure;
the failed step still fails the job. Cancellation stops subsequent steps. The
supervisor forwards termination to its private process group, escalates to KILL,
and removes remaining group members on exit. It does not retry failed assertions.
To reproduce one supervised step after building, run
`HUTERM_CI_SMOKE_STEP=renderer mise run ci:smoke:step`.

The clipboard smokes require tmux on both platforms and `xclip` on Linux. They
start a clean tmux server on a private named socket and never inspect or modify
the user's tmux server or configuration. Linux reads the CLIPBOARD selection
through an independent `xclip` process inside the smoke's private Xvfb display.
The macOS smoke reads exact length-prefixed UTF-8 bytes through a separate
AppKit helper and restores every saved pasteboard item after success or failure.
It bundles the `clipboard_smoke` example in a temporary `.app` and uses the
production desktop startup and Hide command. The helper only observes window
visibility and clipboard contents: external `NSRunningApplication.hide()`
requests were refused by the hosted runner even for a live, regular application.
The tmux shell fixture consumes Huterm's macOS `-l` argument before starting
tmux, while retaining `-c` delegation for commands tmux launches.
Install tmux on macOS with `brew install tmux` before running the native task.
Linux also drives inactive-tab, command-palette, and live permission-reload
writes through native shortcuts. The macOS smoke covers startup denial and
hidden-window delivery; those three shortcut variants still need manual macOS
evidence because the clipboard smoke driver only controls application visibility
and CI does not grant Accessibility event-posting permission.

The hosted macOS runner may choose a different on-screen window origin after
leaving a native fullscreen Space. The smoke requires restored size, style,
focus, PTY geometry, and a settled on-screen frame; verify exact native position
on physical displays. AppKit can also deliver a late screen-change notification
as the next non-native entry begins. The adapter records it, then lets the
main-thread display identity and frame checks distinguish notification noise
from a real display change. Non-native macOS and Linux restoration remain exact.

The pre-commit hook catches what an implementer may have missed, mainly
formatting and lint, plus fast targeted tests. It does not replace running the
relevant tests before committing. Its independent jobs run in parallel and check
staged paths wherever a check can work file by file:

- Rust formatting, Rust test-module order, and Markdown lint receive only the
  matching staged files.
- `mise run test:scripts:affected --fast --staged` runs only the Bun tests
  whose imports, named scripts, or mentioned paths include a staged path,
  including deleted and renamed scripts. It skips the suites listed in
  `SLOW_SUITES` and never runs the whole suite, even for Bun package or
  compiler configuration; run `mise run test:scripts` for those.
- Workflow checks verify only the action pins on staged lines
  (`ci:workflows:staged`), which avoids pinact's GitHub API lookup for every
  pin. Staging `.pinact.yaml` verifies all pins.
- Shell syntax checks parse only staged scripts.

Some checks stay whole-project because a staged file can break unstaged ones.
They run only when a matching input is staged: Clippy for Rust or Cargo
inputs, the protocol boundary for Cargo manifests, TypeScript for scripts,
`lint:agents` for agent guides, and the icon, Sparkle, and vendor checks for
their own inputs. Warm Clippy took 2.5 to 2.8 seconds after an edit in any
crate, because Cargo rechecks only changed crates and their dependents. Harness
configuration validates its own task and hook definitions. Keep the
representative warm path below the project's 10-second hook budget. The hook
reads the working tree, so unstaged edits in the same files can hide or cause a
failure. Dependency audits remain in handoff and CI because they are broader
and may refresh advisory data.

Linux compiles the actual GPUI client and can smoke its window/event loop under
Xvfb with Mesa's software Vulkan device. That smoke does not prove visual
correctness or native input behavior. For macOS evidence, use the
`smoke:macos-*` tasks, which CI also runs on Apple Silicon, and record manual
checks for behavior they do not cover.

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
The task installs both Rust targets, builds each with Ghostty, and uses `lipo`
to assemble the universal executable before packaging. It also
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
directories and runs the exact-byte input smoke with Ghostty.

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

`assets/Huterm.icon` is the icon source. Refresh its committed ICNS, 1024-pixel
PNG, and macOS Assets.car with `mise run icons:generate` using Xcode 27.
Normal builds only run the portable `icons:check`; include `assets/icons.json`
with every regeneration. Xcode 26.3 cannot read this document and actool can
exit zero without producing files. Require fresh outputs from a temporary
directory. Render the PNG with Icon Composer's bundled `ictool`, not xcrun's
unrelated entry point or the compiler's ICNS, which only contains up to 256 pixels.

## Terminal engine

Every terminal uses Ghostty; `mise run dev` starts the app. New configuration
omits `terminal.engine`. See [the engine guide](terminal-engines.md) for legacy
value handling, pinned native inputs, license coverage, and benchmark commands.

`check`, `test`, and `verify` exercise Ghostty and prepare its pinned native
source through Mise. Normal build and packaging tasks do the same.
Standard setup installs the pinned Bun and Zig tools alongside
Rust. Repository scripts run on Bun and are type-checked with TypeScript 7;
Python is not required. Run `mise run scripts:install` to install the locked
TypeScript dependencies, `mise run check:scripts` for script tests and type
checking, and `mise run audit:scripts` for dependency advisories. These checks
also run through the appropriate verification and CI tasks.

### Generated Ghostty bindings

`huterm-ghostty` commits its FFI declarations. `mise run ghostty:bindings`
regenerates them from the pinned headers, and `mise run ghostty:bindings:check`,
part of `mise run check`, fails when they are stale. Both load libclang at run
time and print the version they loaded:

- On macOS the task sets `LIBCLANG_PATH` to the selected Xcode's toolchain, or
  to the Command Line Tools, so the libclang matches the Xcode that builds
  Huterm. Without it, clang-sys prefers any `llvm-config` on `PATH`.
- Ubuntu 22.04 needs `sudo apt-get install --no-install-recommends libclang1-14`.
  The Linux Docker image already provides libclang.
- Set `LIBCLANG_PATH` to the directory containing the library to override
  either choice. A missing library fails with these instructions.

The generator parses the headers as C++17, so libclang 14 and Apple clang 21
produce identical output. CI runs the check only in the macOS arm64 checks
job. That job does not pin Xcode: it builds with the runner image's default,
and the check uses the same Xcode's libclang, so an image update that changes
the output fails the check and names the libclang version in its log. The
Linux runners would load whichever unpinned LLVM the image ships.

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

Ghostty receives input through a raw, no-echo PTY recorder.
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

## Repository scripts and local builds

Repository Rust formatting is defined by `rustfmt.toml`; it must not depend on
or require changes to `~/.rustfmt.toml`.

Use separate Cargo target directories when comparing baseline and feature
worktrees. Reusing release artifacts across them can retain baseline protocol
metadata; clean the affected local crates if a rebuild reports missing symbols
that exist in the current source.

Temporary Git fixtures must clear inherited `GIT_*` variables before invoking
Git. Commit hooks export repository and index paths that override a fixture's
working directory and can redirect its commits into the caller's worktree.

Repository scripts use Bun with TypeScript 7 for type checking, and Bash for the
build wrapper. Pin Bun and Zig in Mise and JavaScript dependencies in bun.lock;
keep bunfig.toml's minimum release age aligned with the three-day policy.

Run `mise run check:scripts` for tooling edits. Native source preparation uses
Bun FFI only for the OS-owned `flock`; retain automatic lock release on process
exit and the existing top-down source-tree hash order. Test changes to extraction
and locking on macOS and Linux. The Linux FFI library is glibc, matching
Ubuntu CI.

Buffer the native archive response before passing it to `Bun.write`. Bun 1.4.0
can stall on Linux when writing the live HTTPS response directly, even though
local HTTP fixtures pass. Verify download changes with a cold preparation run.

Make Bun test tasks that import packages depend directly on `scripts:install`.
A sibling typecheck's install dependency does not order parallel test startup.

Lefthook's `**/*.md` glob skips root Markdown files. Include `*.md` explicitly
so staged README and agent-guide edits receive the same checks as nested docs.

## Tart VM rules

Tart macOS VMs run host-built binaries; the guest never compiles. Stage new
host-built helpers in `scripts/macos-vm/guest.sh`. Stage the
read-only virtiofs share into the guest with `rsync -a`: virtiofs returns ELOOP
for extended attributes on symlinks, so `ditto` and `cp` fail on
`Sparkle.framework`. `tart list` fails while any VM with an ASIF disk is
running, so cleanup cannot enumerate VMs then; `tart get` fails only for the
running VM itself. Background
processes started through `tart exec` die when exec returns; keep guest
commands in the foreground. Virtualization.framework refuses a third running
macOS guest, so runs share two host-wide slot locks.

Smokes and `exec` use disposable clones; `dev` keeps a per-worktree VM. Flush a
kept guest with `tart exec <vm> sync` before stopping it, or recent writes are
lost. `tart clone` onto an existing name silently replaces that VM, so clone only
after `tart get` reports its specific not-found error: `tart get` also fails
for a VM that is merely running, and reading that as absence destroys a kept
guest. Changing the provisioning inputs renames the image and
leaves the previous one on disk until `vm:{macos,linux}:clean` removes it.

Both dev sessions keep one guest instance under host control, with `r` to
rebuild and relaunch, `w` to toggle watching, and `q` to quit. Their Mise tasks
set `raw = true`: Mise otherwise pipes task stdio to prefix output, so the
runner sees no terminal and those keys never arrive. `tart exec` can outlive the
guest process it started, so close the host side after asking the app to stop or
the session hangs on quit. Route SIGINT and SIGTERM through the session, settle
an in-flight rebuild before completing a quit, and let whichever command leaves
last stop a shared VM, whether or not it booted that VM.

Linux Tart VMs run container-built binaries; neither guest compiles. Ubuntu's
GNOME aborts its Wayland session with "No GSettings schemas are installed"
unless provisioning runs `glib-compile-schemas` after installing the desktop,
and GDM then falls back to Xorg silently. GDM selects the session from the
autologin user's AccountsService record, so set it together with WaylandEnable
and restart gdm3. Apple's virtio GPU is not PCI, so Ubuntu's 61-gdm.rules
virtual-GPU checks never match. X can start without working GL while mutter
cannot, which makes a silent Xorg fallback the normal symptom of a broken
Wayland session. Unref a `tart run` child that is deliberately left running, or
Bun's event loop keeps the finished command alive.

Ubuntu desktop ships `/usr/lib/netplan/00-network-manager-all.yaml`, so the
guest needs `network-manager` explicitly under `--no-install-recommends` or
netplan leaves every interface unmanaged and the VM has no network at all.
Shared NAT is sufficient; bridged networking is not required. End guest
provisioning with `sync`, because the image is published as soon as the script
exits and unflushed writes are lost. Prefer regular files under `/etc` for
provisioned overrides: a `systemctl mask` symlink did not survive cloning,
while a unit drop-in did. Bound `systemd-networkd-wait-online`, whose
two-minute timeout otherwise delays every boot before the guest agent answers.

## Docker rules

Mise's Rust install points at `/root/.cargo/bin` in the image; changing
`CARGO_HOME` at runtime makes Mise report Rust missing. Cache Cargo's registry
and Git downloads separately while retaining the image's Cargo home.

The local Linux runner omits Git metadata because linked worktrees reference
paths outside the source mount. It passes host HEAD as HUTERM_SOURCE_REVISION;
benchmark metadata must use that value before falling back to Git.
