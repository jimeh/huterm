# Huterm

Huterm is an experimental terminal emulator and multiplexer written in Rust.
It will start as a native desktop application built with GPUI, but the terminal
runtime will not belong to the desktop UI. Huterm will own its pseudoterminals,
terminal state, sessions, workspaces, tabs, and pane layouts so other clients
can attach to the same runtime later.

The first proof-of-concept implementation is under active validation. It has a
working PTY runtime, incremental terminal snapshots, and macOS/Linux GPUI clients.
Portable core tests, repository checks, Linux native tests, and the Linux Xvfb
smoke pass locally. CI enforces the same suite on Apple Silicon macOS and Linux
x86_64; green current-head CI is required for PR readiness, while manual
visual/input evidence remains before the milestone is complete.

## Direction

Huterm is based on four decisions:

- Huterm owns PTY spawning, I/O, resize, process lifetime, and attachment
  behavior. It will initially use `portable-pty` for the operating-system
  implementation.
- The runtime owns canonical terminal state and the shared scroll position.
  Upstream `alacritty_terminal` is the default engine; every build also
  provides `libghostty-vt` behind the same snapshot and input boundary.
- Clients render Huterm-owned terminal snapshots. The first client uses GPUI;
  a text-based terminal client can use the same model later.
- Huterm application code targets the MIT license. Dependencies may use other
  compatible permissive licenses. GPL-covered Zed application code is outside
  the project boundary.

Both engines stay behind the runtime's private emulator module and expose the
same Huterm-owned snapshot and input boundary. PTY ownership, workspaces, and
GPUI rendering are shared.

## Current desktop

Huterm runs on macOS Apple Silicon and Linux x86_64. Each native window has a
private session and backing workspace with ordered tabs, one pane per tab,
and independent terminal processes. Tabs can appear at the top, bottom, left,
or right. Hidden tabs keep processing output without preparing viewport
snapshots or painting.
Drag tabs to reorder them within a window. The preview stays in the tab bar
even when the pointer leaves the window; releasing commits the clamped
insertion position. Escape cancels. Drag near a bar edge to scroll toward
hidden tabs. Horizontal tabs share the window width, shrinking to 120 logical
pixels before scrolling. Vertical tabs keep a fixed height in a resizable
sidebar. Scroll the bar with a trackpad or mouse wheel; floating arrows show
where more tabs remain. The new-tab button stays visible and follows the last
tab in vertical mode. Moving tabs between windows and tearing tabs out are not
supported.

Closing a tab stops its terminal. Windows attach to sessions. Closing a window
only detaches when another view remains; closing its final view terminates that
session. Core detachment and attachment retargeting preserve zero-view sessions.
Huterm stays alive without windows while any session survives, and macOS Dock
reopen creates a window. Hiding a window keeps its attachment.

Close and Quit warn about foreground/background jobs or unknown process state.
Idle shells need no confirmation. Structure and job evidence are rechecked before
teardown. Consent covers existing job groups while their leader creation identity
survives, including leader exec and child churn. A leaderless group needs an
original surviving member. New groups, lost identity evidence, structural changes,
or newly unknown process state require reassessment and renewed consent when
needed. Completed jobs do not re-prompt. macOS Dock Quit uses the same cancellable
path as the application menu.
Quit includes all sessions, even those with no views. Before teardown it retains
one in-memory hierarchy and window-navigation/layout capture. Disk persistence,
terminal-history serialization, and restart restoration remain unimplemented.
Tabs close automatically without confirmation when their root shell exits.
Set `[terminal] close_on_exit = false` to retain exited tabs for history. Retained
tabs allow selection, copy, scrolling, and resizing, but send no terminal input.
Reloading this setting affects future exit events only.

Keyboard and application mouse input, paste, selection and copy, runtime-owned
scrollback, configurable fonts and themes, native fullscreen, and alternate-screen
applications are supported. A background server, local IPC, workspace switching,
split panes, and a TUI client remain follow-up work.

See the [workspace plan](docs/plans/workspaces-windows-tabs.md) for the ownership
boundaries and follow-up milestones, and the
[initial desktop plan](docs/plans/initial-desktop-poc.md) for the original scope.

## Core ownership

One runtime owns a logical socket scope, ordered sessions, ordered workspaces,
and ordered tabs. The socket name defaults to `default`; this does not create a
socket file or listener. Core supports custom names, clearing name overrides,
ID-targeted selection validation, tab transfers between workspaces, and workspace
transfers between sessions. Moves preserve pane and terminal identities, live
processes, and attachments. Names can be duplicated; structural IDs include the
runtime incarnation so equal numeric IDs in different runtimes cannot alias.

These are core APIs. The desktop still creates a private session per window;
rename controls, cross-window transfers, session/workspace switching, shared
views, and persistence remain follow-up work. The embedded attachment and close
lifecycle is implemented; server-mode lifetime policy and IPC remain deferred.

Process inspection uses a bounded system `ps` snapshot off the UI and terminal
parser threads. It follows shell descendants and the owned PTY, including
background process groups while the root shell is alive. Scan failures require
confirmation. Root-shell exit completes the terminal and reaps the child
immediately, including when its tab is retained for history. Closing completed
history does not scan for or signal surviving processes. This follows the
root-exit policy used by other desktop terminals; a surviving background job
does not keep the terminal open or trigger a warning after shell exit.
Detached processes cannot always be attributed or terminated. Process creation
or identity changes after the final live-process snapshot are inherently racy.

## Mouse interaction

Applications can request button reports with mode 1000, held-button motion with
1002, or all motion with 1003. Huterm supports left, middle, and right buttons,
vertical and horizontal wheels, and legacy, UTF-8 1005, and SGR 1006 reports.
Control and Alt/Option modifiers are included. Applications that do not enable
reporting receive no mouse input.

Hold Shift when starting a selection or scrolling to use local terminal
selection and scrollback. The scrollbar always stays local. Mouse input also
stays local while viewing scrollback, including while a scroll away from live
is waiting for its snapshot. Return to the live viewport to interact with the
application again. A drag keeps its original ownership until release, even if
Shift changes or the pointer leaves the grid.

Leaving the live viewport or losing focus ends application button gestures.
Coordinates for continuing gestures clamp to the grid edge. Fast application
wheel input is limited to 32 steps per platform event; excess whole steps and
wheel steps that cannot fit the input queue are discarded. Legacy reports clamp
to column/row 223 and UTF-8 reports to 2015; SGR uses the full terminal grid.

## Development

Install the pinned tools and local hook:

```sh
mise run setup
```

On macOS or Linux, launch the app with:

```sh
mise run dev
```

Huterm reads `$HUTERM_CONFIG_FILE` when set, otherwise
`$XDG_CONFIG_HOME/huterm/config.toml` or `~/.config/huterm/config.toml`. The
Settings command creates a documented default file without replacing an
existing one, then opens it in the system editor.

Terminal padding defaults to 4 logical points on each side. Add or adjust the
`[window]` section to change it:

```toml
[window]
padding_x = 4.0
padding_y = 4.0
padding_balance = false
tab_position = "top" # top, bottom, left, or right
```

Set `padding_balance = true` to split leftover horizontal space evenly between
left and right when the window width does not fit whole columns. With it off,
the remainder stays on the right. Vertical remainder always stays at the
bottom. Padding accepts values from 0 to 256 points.

### Themes and config reload

Select a built-in theme and optionally override individual colors:

```toml
[theme]
name = "catppuccin-mocha"
background = "#181825"
ansi_red = "#f38ba8"
ansi_bright_red = "#eba0ac"
```

Built-ins are `huterm-dark` (the unchanged default), `tokyo-night`,
`tokyo-night-storm`, `tokyo-night-moon`, `tokyo-night-day`,
`catppuccin-latte`, `catppuccin-frappe`, `catppuccin-macchiato`,
`catppuccin-mocha`, `dracula`, `nord`, `one-dark-pro`, `tomorrow-night`,
and `tango-with-monokai`.
They are embedded in the application; no downloads are needed.

Define a named theme directly in your config:

```toml
[theme]
name = "my-mocha"

[themes.my-mocha]
extends = "catppuccin-mocha"
background = "#181825"
```

Or place it in `themes/my-mocha.toml` next to your config file:

```toml
[theme]
extends = "catppuccin-mocha"
background = "#181825"
```

Names use letters, digits, hyphens, and underscores, without a file extension.
Lookup prefers inline definitions, then adjacent theme files, then built-ins.
The first match wins; same-name definitions do not merge. Use a distinct custom
name when extending a built-in. Without `extends`, a definition starts from
Huterm Dark. Overrides in the config's `[theme]` table apply last.

All color keys are flat: `foreground`, `background`, `cursor`, `selection`
(selection background), optional `selection_foreground`, and `ansi_black`,
`ansi_red`, `ansi_green`, `ansi_yellow`, `ansi_blue`, `ansi_magenta`,
`ansi_cyan`, `ansi_white`, with corresponding `ansi_bright_*` keys.
Colors use `#rrggbb`. The legacy 16-color `ansi = [...]` array still works;
individual ANSI keys override its entries. Unspecified fields inherit.
Without `selection_foreground`, selected text retains its original colors.
Labels and scroll controls derive their colors from the theme.

Use **Reload Configuration** in the Huterm menu, or press Cmd+Shift+, on
macOS / Ctrl+Shift+, on Linux with a US keyboard layout. GPUI represents these
as Cmd+< / Ctrl+<; on other layouts use the corresponding less-than chord.
It rereads the config and selected theme chain
and applies font, padding, and colors without restarting the shell or losing
scrollback. All open windows and retained tabs receive the reload. Windows keep
their size;
visible grids are recalculated and hidden grids resize when selected.
Invalid configuration leaves the running settings intact and displays an error.
An unreadable or missing config file does the same. To reset to defaults,
empty the config file and reload it.
Explicit colors set by terminal applications remain intact. Reload is manual;
there is no file watcher or remote CLI command.

See [theme sources and licenses](crates/huterm-gpui/themes/README.md) for
attribution and palette import details.

On Apple Silicon macOS, build the application bundle with:

```sh
mise run package:macos
```

Use `mise tasks` to discover all commands. `mise run check` is the fast local
gate, while `mise run verify` also runs tests, the dependency-license policy,
and GitHub Actions checks. See the
[development guide](docs/agents/development.md) for platform prerequisites,
limits, and the validation ladder.

## Planned features

The following list describes the intended product direction. Items outside the
current milestone are not yet scheduled.

The [workspace, window, and tab plan](docs/plans/workspaces-windows-tabs.md)
records the next proposed PR and the path toward workspace switching,
restoration, and shared GUI/TUI access.

### Terminal

- Local shells and arbitrary commands with configurable working directory and
  environment.
- More complete grapheme handling, font fallback, and IME.
- True color, text decorations, cursor styles, hyperlinks, clipboard support,
  search, selection, and configurable scrollback.
- More terminal mouse protocols, configurable keybindings, and shell
  integration.
- Configurable Alacritty and `libghostty-vt` terminal engines, with Alacritty
  selected by default.

### Desktop clients

- Native macOS, Linux, and Windows applications built with GPUI.
- Horizontal or vertical tab bars.
- Tabs containing arbitrary horizontal and vertical pane splits.
- Native fullscreen on supported platforms.
- Borderless non-native fullscreen on macOS without creating a separate
  Mission Control space.
- Moving tabs between windows, notifications, and restored window layouts.
- Accessibility and platform-native input behavior.

### Multiplexer

- A local server that owns terminal processes independently of client windows.
- Named workspaces containing tabs and split-pane layouts.
- Detach and reattach without terminating terminal processes while the server
  remains alive.
- Multiple simultaneous clients with explicit terminal-size policy.
- A text-based client that can run inside another terminal.
- A command-line client for workspace and pane automation.
- Versioned local IPC, followed by optional authenticated remote attachment if
  the local design proves useful.

### State and recovery

- Persisted configuration, workspace metadata, tab order, and pane layouts.
- Scrollback and presentation-state recovery where it can be made reliable.
- Clear distinction between restoring saved presentation and preserving a live
  child process. A server crash is allowed to terminate its PTYs.

## Terminology

- **Socket**: a named runtime scope containing sessions.
- **Session**: a runtime-owned collection of ordered workspaces within a socket.
- **Workspace**: a collection of ordered tabs within a session, independent of
  its views.
- **Tab**: an ordered container with one pane layout.
- **Pane**: a leaf in a tab's split tree.
- **Terminal**: a PTY, child process, terminal-emulator state, scrollback, and
  shared viewport.
- **Client**: a desktop, TUI, or command-line connection to the runtime.
- **Client view**: client-local focus, active tab, selection gestures, and
  presentation preferences.

Tab order and pane layout belong to the workspace. Each window or TUI view
selects its workspace independently and owns focus and the selected tab.
Terminal scroll position is shared across views for the engine experiment.
Tab-bar orientation is a client preference, not workspace state. See
[CONTEXT.md](CONTEXT.md) for the full glossary.

## Terminal engines

Every build includes both engines, with Alacritty as the default. Run
`mise run dev`, then choose the engine for new terminals in your configuration:

```toml
[terminal]
engine = "ghostty" # Or "alacritty".
```

Reloading configuration changes subsequently created tabs and windows. Existing
terminals retain their captured engine, child process, and history. Pending
terminal creation retains its captured engine choice across reloads.
Unknown engine names are explicit configuration errors. Normal build tasks
prepare verified Ghostty source and use the pinned Zig toolchain.

Each terminal owns one shared viewport. Snapshots remain complete and immutable;
unchanged rows share storage between generations. Both engines use the existing
GPUI renderer. See [the engine guide](docs/agents/terminal-engines.md) for
native build inputs, benchmarks, and known engine differences.

## Licensing

Huterm intends to license its own source under the MIT license. The planned
initial dependencies include MIT and Apache-2.0 software. Distributed builds
will retain the required third-party license notices and use an automated
license allowlist to prevent accidental GPL or AGPL dependencies.

Huterm-owned source is available under the [MIT license](LICENSE). The committed
dependency policy audits the full resolved graph and rejects GPL, AGPL, unknown
registries, Git dependencies, and wildcard Cargo requirements.
