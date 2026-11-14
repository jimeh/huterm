# `huterm` command-line entry point (#350)

Status: implemented. Plans [issue #350][issue-350] (parent [#37][issue-37]).
Later subcommands build on it: the SSH helper ([#351][issue-351]), server
control ([#45][issue-45]), and CLI control of a running app ([#46][issue-46]).

## Outcome

`huterm` is a small command-line program. Run without a subcommand, it opens
the desktop app as running the app's executable from a shell did before. With
arguments it parses them, runs the subcommand, and exits without loading GPUI,
opening a window, or needing a display. It ships three ways:

- inside the app: `Huterm.app/Contents/MacOS/huterm` and the Linux packages'
  `bin/huterm`;
- on `PATH` inside Huterm terminals, through shell integration's `path`
  feature, which is now on by default; and
- as its own release download per platform, for machines without the app.

The first release has only `--help`, `--version`, and usage errors. Later
subcommands follow the conventions below.

## Decision: a separate CLI binary, and the GUI becomes `huterm-gui`

The CLI is a new `huterm-cli` crate whose binary is named `huterm`. The desktop
executable, built from the root `huterm` package, is renamed `huterm-gui`.
Running `huterm` without arguments replaces itself with the `huterm-gui` that
sits beside it, so the process ID, standard streams, and exit status stay the
desktop app's.

A subcommand mode inside the GUI binary was rejected:

- **Standalone use.** The Linux GUI executable names `libxcb.so.1`,
  `libXau.so.6`, and `libXdmcp.so.6` as host libraries and loads the private
  `libxkbcommon` family through its `RUNPATH`
  (`assets/linux/package-policy.json`). The dynamic loader refuses to start it
  on a host without them, before `main` runs, so a GUI binary cannot be the
  headless standalone download. A second, GPUI-free binary must exist anyway.
- **One binary everywhere.** With the split, the bundled CLI, the Linux
  packages' CLI, and the standalone download are the same bytes per platform
  and behave the same. A dispatching GUI binary plus a standalone CLI would be
  two programs named `huterm` with different behavior.
- **Launch arguments.** The GUI accepts whatever LaunchServices, `open
  --args`, Sparkle, and the smokes pass, as today. Argument parsing never runs
  in the GUI process, so a `-psn_…` or `-NSDocumentRevisionsDebugMode YES`
  argument cannot become a usage error that stops the app from opening.
- **Size and startup.** Measured on this branch (macOS arm64 release
  builds, 2026-11-14): `huterm-gui` is 37.1 MB and links 18 system
  libraries and frameworks, including AppKit, Metal, QuartzCore, and
  CoreText. `huterm` is 0.98 MB and links only `libSystem`, and
  `huterm --version` takes a median 4.6 ms over 30 runs, including the
  measuring process's spawn.

The cost is the rename. WezTerm uses the same split (`wezterm` and
`wezterm-gui`).

### Launch compatibility

| Launch path | Before | After |
| --- | --- | --- |
| Finder, Dock, `open`, LaunchServices, login items, `open --args` | `CFBundleExecutable` `huterm` | `CFBundleExecutable` `huterm-gui`, which accepts any arguments as before. The bundle path, identifier, and app signature identity are unchanged, so TCC grants and Dock items are expected to carry over |
| Sparkle update relaunch | relaunches the bundle at its path | expected unchanged: Sparkle relaunches the installed bundle and reads the new `Info.plist`. Unverified until the first update from a release whose executable was `huterm` |
| `Huterm.app/Contents/MacOS/huterm` from a shell | GUI | CLI, which replaces itself with `huterm-gui`; arguments now go to the CLI |
| Linux desktop entry `Exec=huterm` | GUI | CLI, which replaces itself with `huterm-gui` |
| AppImage `AppRun` → `usr/bin/huterm` | GUI | CLI, which replaces itself with `huterm-gui`; `Huterm.AppImage --help` now prints help |
| Tarball `bin/huterm` | GUI | CLI, as above |
| `mise run dev`, smokes, benches, VM staging and cleanup | `target/*/huterm`, `pkill -x huterm` | `target/*/huterm-gui`, `pkill -x huterm-gui`; `mise run dev` also builds `huterm` |

The CLI finds the GUI next to its own canonicalized path, so a symlink such as
`/usr/local/bin/huterm` → `/Applications/Huterm.app/Contents/MacOS/huterm`
still finds `huterm-gui`. A standalone CLI without a `huterm-gui` beside it
reports that the desktop app is not installed next to it and exits 1.

## Conventions for subcommands

- **Parsing:** `clap` 4 with derive, already in `Cargo.lock` at 4.6.7
  (MIT or Apache-2.0). Each subcommand is a variant of `Command` with a doc
  comment as its help, and its arguments and `run` function live in their own
  module under `crates/huterm-cli/src/commands/`. A group such as
  `huterm server <action>` is a variant holding a nested subcommand enum.
- **Help:** `--help` and `-h` everywhere; `huterm help <command>` once a
  subcommand exists. Help and `--version` print to standard output and exit 0.
- **Usage errors:** an unknown subcommand, an unknown or missing argument, or
  an invalid value prints clap's error and usage to standard error and exits
  2.
- **Failures:** a subcommand that parsed but failed prints
  `huterm: <message>` to standard error and exits 1. Results go to standard
  output; diagnostics go to standard error.
- **Boundaries:** `huterm-cli` must not depend on `gpui-pre`, `huterm-gpui`,
  or `huterm-ui`; `mise run architecture` enforces it. A subcommand may link `huterm-client`,
  `huterm-server`, or `huterm-core`, so #45 can host a server from the same
  binary on a headless machine.
- **No subcommand:** opens the desktop app. Subcommands never start it
  implicitly.

## Packaging and release

- **macOS app:** `cargo-packager` copies both executables into
  `Contents/MacOS`, with `huterm-gui` as the main binary. Both are universal.
  Release signing signs `huterm` with the hardened runtime, a secure
  timestamp, the identifier `app.huterm.dev.cli`, and no entitlements, before
  the app.
- **Standalone macOS:** `Huterm-CLI-<version>-macOS-universal.zip` holds
  `Huterm-CLI-<version>-macOS-universal/` with the same signed `huterm` bytes,
  `LICENSE`, and `licenses.json`. It is submitted to notarization on its own.
  Apple cannot staple a ticket to a bare executable, so Gatekeeper checks it
  online on first run.
- **Linux packages:** `bin/huterm` (CLI) and `bin/huterm-gui` (GUI, with the
  private library `RUNPATH`). `AppRun` and the desktop entry are unchanged.
- **Standalone Linux:** `Huterm-CLI-<version>-Linux-<arch>.tar.gz` holds
  `Huterm-CLI-<version>-Linux-<arch>/` with `huterm`, `LICENSE`, and
  `licenses.json`. The CLI needs only glibc 2.35 or newer.
- **Release inventory:** 13 files instead of 10. The publish step appends a
  short "Downloads" section to the draft's notes that names the app and CLI
  assets.
- **Notices and SBOM:** `licenses.json` also covers `huterm-cli`'s dependency
  tree. The macOS SBOM merges each architecture slice's crates from both
  executables and requires `huterm-cli` at the release version.
- **Versions:** `huterm-cli` carries the scalar workspace version with a
  `# x-release-please-version` dependency line, and release validation checks
  it with the other packages.
- **Updater build:** `build.rs` links Sparkle into `huterm-gui`, and the
  updater package verifier inspects `huterm-gui`.
- **PR CI:** the existing `package` jobs build and verify the CLI archives
  (unsigned on macOS) beside the app packages.

## Non-goals

- Any subcommand beyond `--help` and `--version`, shell completions, and man
  pages.
- Opening the app through LaunchServices from a standalone CLI, and a
  `huterm` that talks to a running app.
- A Homebrew formula, `.deb`, or `.rpm`; documentation covers symlinks and
  the standalone download.
- A musl or older-glibc Linux CLI.

## Verification

| Concern | Evidence |
| --- | --- |
| `--help`, `--version`, unknown subcommand and argument: output stream, text, exit status, no display, and a sibling app stub that never runs | `huterm-cli` unit tests and integration tests that run the built binary with `DISPLAY` and `WAYLAND_DISPLAY` removed |
| No subcommand replaces the CLI with the sibling `huterm-gui`: same PID, the app's streams and exit status, no arguments, through a symlink too | Integration tests with a stub `huterm-gui` script in a temporary directory |
| A missing or unexecutable `huterm-gui` exits 1 with `huterm: <message>` | Integration tests |
| The CLI has no GPUI dependency | `mise run architecture` |
| Linux packages: both executables and modes, the CLI's C-runtime-only `NEEDED` set and absent runpath, CLI conventions without a display, desktop launch through `bin/huterm` and `AppRun` | `package-linux.ts` verification and package smokes, in PR CI and `package:linux:container` |
| Standalone Linux tarball: entries, modes, notices, identical bytes to `bin/huterm`, conventions | `package-linux.ts` verification, PR CI |
| Linux desktop smoke launches through `huterm` | CI `linux-desktop` smoke step |
| macOS app: `CFBundleExecutable`, both executables universal with the macOS 14 floor | `package:macos` locally and in PR CI; `package:macos-release` locally for the updater build |
| Standalone macOS zip: entries, modes, notices, identical bytes to the bundled `huterm`, libSystem-only linkage, conventions | `package:macos` locally and in PR CI |
| Signing plan signs the CLI first with its identifier and no entitlements; 13-asset inventory, assembly, workflow staging lists, release body, SBOM slices | Bun unit tests, with perturbations of the staging and SBOM checks |
| Finder, `open`, and `open --args` launch `huterm-gui`; `huterm --version` resolves inside a packaged app's terminal | Codex computer use on a scratch bundle |
| `path` feature on by default | Config unit test; the existing native shell tests cover appending `HUTERM_BIN_DIR` |

Not covered before a release: Developer ID signing and separate notarization
of the CLI, and the first Sparkle update from a release whose executable was
`huterm`, including whether TCC grants carry over. Check these by hand on the
first release that carries this change. The CLI's notarization reuses the
app's submission helper, which fails the release on any status other than
`Accepted`.

[issue-37]: https://github.com/jimeh/huterm/issues/37
[issue-45]: https://github.com/jimeh/huterm/issues/45
[issue-46]: https://github.com/jimeh/huterm/issues/46
[issue-350]: https://github.com/jimeh/huterm/issues/350
[issue-351]: https://github.com/jimeh/huterm/issues/351
