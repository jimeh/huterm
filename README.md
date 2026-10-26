# Huterm

Huterm is a terminal emulator and multiplexer written in Rust. Its runtime owns
the pseudoterminals, terminal state, sessions, workspaces, tabs, and panes, so
the native desktop app is one client of that runtime rather than its owner. A
local server, a text client, and a browser client are planned on the same
runtime.

Huterm is pre-1.0 and changes quickly. Signed macOS builds and Linux packages
are published on the [releases page](https://github.com/jimeh/huterm/releases),
and macOS builds update themselves. The desktop app runs on macOS 14 or later
and on Linux x86_64 and aarch64 under X11 or XWayland. Every terminal uses
[Ghostty](https://ghostty.org)'s `libghostty-vt` for terminal emulation and
GPUI for rendering.

## What works today

- **Terminals:** 24-bit color, runtime-owned scrollback with configurable row
  and size limits, selection, copy and
  paste, application mouse reporting, clickable links, file drops, clipboard
  writes from applications (OSC 52, OSC 1337 `Copy`, and Kitty's OSC 5522),
  color and geometry queries, a visual bell, and a private `xterm-huterm`
  terminfo entry.
- **Windows and tabs:** several windows, each with ordered tabs in a top,
  bottom, left, or right bar or merged into the title bar. Tabs can be
  reordered by dragging, renamed, and labelled by program, title, or working
  directory.
- **Commands:** every action is a catalog command with typed arguments. The
  command palette finds commands by fuzzy search and prompts for arguments, and
  keybindings can bind any command with arguments and `when` conditions.
- **Presentation:** native fullscreen, non-native fullscreen on macOS, and
  quake windows summoned by global shortcuts on macOS and native X11.
- **Safe close and Quit:** Huterm inspects each terminal's processes and asks
  before closing anything with running jobs. An idle shell closes without a
  prompt.
- **Configuration:** a TOML file with a published JSON Schema, 14 built-in
  themes, custom themes, and reload without restarting shells.

These are not available yet:

- Split panes. Each tab holds one pane.
- Switching sessions or workspaces in a window, and moving tabs between
  windows. Each window gets a private session.
- A background server, local IPC, a command-line client, a TUI, or a browser
  client.
- Saving and restoring sessions or terminal history across restarts.
- Shell integration, desktop notifications, inline graphics, and window
  transparency.

[Roadmap](#roadmap) summarizes the plan for these.

## Install

Download a build from the
[releases page](https://github.com/jimeh/huterm/releases). Each release
publishes a `SHA256SUMS` file; check downloads against it.

### macOS

Download `Huterm-<version>-macOS-universal.zip`, unzip it, and move
`Huterm.app` to Applications. The app is a universal binary, signed with a
Developer ID and notarized by Apple, and needs macOS 14.0 or later. Intel Macs
are supported by the build but have not been validated on Intel hardware.

Release builds update themselves with [Sparkle](https://sparkle-project.org).
Choose **Check for Updates...** from the Huterm application menu to check now.
On the second launch, Sparkle asks whether to check automatically; if you
agree, it checks every 24 hours. The update window lists the release notes for
every version between the installed one and the update. The `[updates]`
settings in [Configuration](docs/configuration.md#updates) can manage scheduled
checks instead.

### Linux

Each release has two packages for x86_64 and for aarch64:

- `Huterm-<version>-Linux-<arch>.AppImage` runs directly. Make it executable
  first. If FUSE is unavailable, run it with `--appimage-extract-and-run` or
  use the tarball.
- `Huterm-<version>-Linux-<arch>.tar.gz` holds the same payload for manual
  installation: `bin/huterm`, a desktop entry, AppStream metadata, an icon,
  and license notices. It has no AppImage runtime.

The packages need glibc 2.35 or newer (Ubuntu 22.04 or later), X11 or
XWayland, and a working Vulkan driver. GNOME on Wayland runs Huterm through
XWayland. Linux builds do not update themselves.

## Configuration

Huterm reads `$HUTERM_CONFIG_FILE` when it is set. Otherwise it reads
`$XDG_CONFIG_HOME/huterm/config.toml`, falling back to
`~/.config/huterm/config.toml`, on both macOS and Linux. The **Open Settings**
command (`open_settings`) creates a documented default file if none exists and
opens it in the system editor. Add this header to get completion and validation
from the published JSON Schema in Taplo or the Even Better TOML editor
extension:

```toml
#:schema https://github.com/jimeh/huterm/releases/latest/download/huterm.schema.json
```

**Reload Configuration** (Cmd+Shift+, on macOS and Ctrl+Shift+, on Linux, with
a US keyboard layout) applies changes to every open window without restarting
shells. An invalid file leaves
the running settings in place and shows an error.

- [Configuration](docs/configuration.md) covers every setting: tabs and
  windows, fonts and themes, quake windows, links, clipboard writes, terminal
  identity, and updates.
- [Commands and keybindings](docs/keybindings.md) covers the command palette,
  keybinding syntax, every command, and the default bindings.

## Windows, sessions, and tabs

Each window creates and attaches to a private session with one workspace of
ordered tabs. Each tab holds one pane with its own terminal and shell. Hidden
tabs keep processing output without building snapshots or painting. A tab closes
when its shell exits, unless `close_on_exit = false` keeps it for its history;
see [Shells](docs/configuration.md#shells). The configuration guide also covers
[the tab bar](docs/configuration.md#tab-bar),
[fullscreen](docs/configuration.md#fullscreen), and
[quake windows](docs/configuration.md#quake-windows).

### Closing windows and quitting

Closing a tab stops its terminal. Closing a window closes its session and
stops its terminals. A window closes on its own when its last tab goes away,
for example when its shell exits; that close stops no other terminal and never
asks.

Close and Quit warn about foreground or background jobs, or when Huterm cannot
determine a terminal's process state. Idle shells need no confirmation. Huterm
checks again before it tears anything down. Consent covers the job groups that
existed when you confirmed, as long as their leaders survive, even if a leader
runs `exec` or its children change. New job groups, lost process evidence,
structural changes, or newly unknown process state lead to a new assessment
and, where needed, a new prompt. Completed jobs do not prompt again. A terminal
whose runtime stopped after an internal failure closes without a prompt; its
runtime has already stopped its shell.

Huterm reads process facts in-process, off the UI and terminal threads. It
follows the shell's descendants and processes that hold the terminal, including
background process groups while the shell is alive. When the shell exits,
Huterm treats the terminal as finished, like other desktop terminals: a
surviving background job does not keep the tab open or cause a warning.
Detached processes cannot always be attributed or terminated.

Quit covers every session, and the Dock's Quit on macOS uses the same
cancellable path as the application menu. Huterm keeps running without windows
while a session survives or a quake shortcut is registered, and the macOS Dock
icon reopens a window. Otherwise it exits when the last window closes.

The runtime and desktop already handle several windows on one session: closing
one of those windows only detaches it, without a prompt, and the session's last
window closes the session. No command opens a second window on a session yet;
that arrives with session switching.

## Develop

Huterm uses [Mise](https://mise.jdx.dev) for its tools and tasks. Run
`mise tasks` to list them all.

Building on macOS needs macOS 14 or later and Xcode with Apple's Metal
Toolchain component. Building on Linux needs the X11 and XKB development
libraries and a Vulkan driver. The
[development guide](docs/agents/development.md) lists the packages, and
`mise run doctor` checks for them.

Install the pinned tools and the pre-commit hook, then start the app:

```sh
mise run setup
mise run dev
```

The main checks are:

- `mise run check`: formatting, Clippy, test-module order, documentation, and
  architecture boundaries. It is the fast local gate.
- `mise run test`: Rust unit and PTY integration tests, plus the repository
  script tests.
- `mise run verify`: the broad check that matches CI, adding the
  dependency-license policy and workflow checks.
- `mise run linux:test` and `mise run linux:smoke`: Linux tests and desktop
  smokes in Docker, from macOS or Linux.
- `mise run smoke:macos-*` and `mise run smoke:linux-*`: native desktop smokes
  for input, the palette, fullscreen, quake windows, and more.

GitHub Actions runs the checks and desktop smokes on macOS arm64 and on Linux
x86_64 and aarch64, where the smokes run under Xvfb.

To build packages locally:

- `mise run package:macos` builds a universal `Huterm.app` in
  `target/release/bundle/`. It leaves out Sparkle and the update feed, and it
  is not signed or notarized.
- `mise run package:linux:container` builds verified AppImage and tarball
  packages in the pinned Ubuntu 22.04 container and exports them to `dist/`.

Set `HUTERM_SOURCE_REVISION` to the source commit when building to show its
short form in the About panel. The container-based Linux runner passes the
host's `HEAD`; builds without it omit the revision.

Release Please maintains a release pull request from Conventional Commit
titles. Merging it tags the release, and the release workflow builds, signs,
notarizes, and publishes the macOS and Linux packages. The
[release guide](docs/agents/releases.md) covers the configuration and recovery
steps.

[AGENTS.md](AGENTS.md) and the guides in [docs/agents](docs/agents/) record
the architecture rules and platform hazards. Plans in [docs/plans](docs/plans/)
record design decisions as they were made.

## Roadmap

Huterm is heading toward a desktop terminal with an optional persistent
multiplexer. In the default embedded mode, the runtime and the desktop share
one process. An opt-in named server will keep sessions alive across client
restarts and let several clients attach to them: desktop windows, a TUI, a
command-line client, and a browser client.

The groundwork is in place: runtime-owned sessions, the command catalog with
the palette and configurable keybindings, fullscreen and quake windows, and
viewers that let several windows show one terminal with a shared viewport. The
next stages are:

- **Desktop expansion:** session and workspace switching, split panes and
  layouts, moving tabs between windows, shell integration, a settings UI,
  window transparency, and restoring sessions and history after a restart.
- **Terminal compatibility:** notifications and progress, extended underlines,
  SGR-pixel mouse reports, the Kitty keyboard protocol, and inline graphics.
- **Optional server:** versioned local IPC, named servers, CLI control, and
  attaching the desktop to a server.
- **More clients:** a browser client, a TUI, and remote attachment.

Restoration after a restart recreates the structure and history with fresh
shells. It does not resurrect running processes; only reattaching to a server
that is still running keeps them.

[Issue #8](https://github.com/jimeh/huterm/issues/8) tracks the roadmap and the
live status of each epic.

## Architecture

Huterm rests on a few decisions:

- The runtime owns PTY spawning, I/O, resize, process lifetime, and attachment.
  It uses `portable-pty` for the operating-system layer.
- The runtime owns each terminal's canonical state and its shared viewport.
  Every terminal uses `libghostty-vt` through Huterm's own `huterm-ghostty`
  binding, behind a private engine boundary. Native emulator types never leave
  `huterm-core`.
- Clients render immutable Huterm snapshots. Each client view registers a
  viewer for every terminal it shows, and the runtime arbitrates input, mouse
  gestures, and terminal size between viewers. The terminal follows the size
  of the viewer that most recently gained focus or typed.
- `huterm-protocol` holds the dependency-neutral IDs, input, events, and
  snapshots that clients use. It exposes no GPUI, Ghostty, or `portable-pty`
  types, so a server and other clients can reuse it.

One runtime owns a logical socket scope with ordered sessions, workspaces, and
tabs. The socket name defaults to `default`; there is no socket file or
listener yet. Sessions, workspaces, and tabs can have custom names, which may
repeat. The core can move tabs between workspaces and workspaces between
sessions while keeping their terminals, processes, and attachments. The
desktop exposes renaming but not these moves yet.

[CONTEXT.md](CONTEXT.md) defines the project's terms: socket, session,
workspace, tab, pane, terminal, attachment, viewer, and more.

## License

Huterm's own source is available under the [MIT license](LICENSE). Huterm does
not use GPL-covered Zed application code. The dependency policy in `deny.toml`
audits the full resolved dependency graph, allows only listed licenses such as
MIT and Apache-2.0, and rejects GPL and AGPL licenses, unknown registries, Git
dependencies, and wildcard Cargo requirements.
