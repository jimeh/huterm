# Huterm

Huterm is an experimental terminal emulator and multiplexer written in Rust.
It will start as a native desktop application built with GPUI, but the terminal
runtime will not belong to the desktop UI. Huterm will own its pseudoterminals,
terminal state, sessions, tabs, and pane layouts so other clients can attach to
the same runtime later.

The first proof-of-concept implementation is under active validation. It has a
working PTY runtime, Alacritty-backed snapshots, and macOS/Linux GPUI clients.
Portable core tests, repository checks, Linux native tests, and the Linux Xvfb
smoke pass locally. CI enforces the same suite on Apple Silicon macOS and Linux
x86_64; green current-head CI is required for PR readiness, while manual
visual/input evidence remains before the milestone is complete.

## Direction

Huterm is based on four decisions:

- Huterm owns PTY spawning, I/O, resize, process lifetime, and attachment
  behavior. It will initially use `portable-pty` for the operating-system
  implementation.
- The runtime owns canonical terminal state. It will initially use upstream
  `alacritty_terminal` for escape-sequence parsing, the terminal grid,
  scrollback, modes, and cursor state.
- Clients render Huterm-owned terminal snapshots. The first client uses GPUI;
  a text-based terminal client can use the same model later.
- Huterm application code targets the MIT license. Dependencies may use other
  compatible permissive licenses. GPL-covered Zed application code is outside
  the project boundary.

The terminal engine is replaceable in principle, but Huterm will not build a
generic backend framework before a second engine exists. A future
`libghostty-vt` experiment should replace the private emulator module without
changing PTY ownership, sessions, client messages, or renderers.

## Initial build target

The first milestone targets macOS on Apple Silicon, with Linux x86_64 as an
additional development platform. It opens one GPUI window containing one tab,
one pane, and one interactive local shell. Huterm owns the PTY and Alacritty
terminal state, then renders a Huterm-defined snapshot in GPUI.

The milestone includes keyboard input, paste, selection and copy, fast
client-owned scrollback with a position indicator, configurable font and theme,
terminal resize, native fullscreen, alternate-screen applications, a macOS menu
bar and Apple Silicon application bundle, and clean child-process shutdown. It
does not include a background server, local IPC, multiple tabs, split panes, or
a TUI client.

See the [initial desktop proof-of-concept plan](docs/plans/initial-desktop-poc.md)
for the implementation sequence and acceptance criteria.

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
macOS / Ctrl+Shift+, on Linux. It rereads the config and selected theme chain
and applies font, padding, and colors without restarting the shell or losing
scrollback. The window stays the same size; its grid is recalculated.
Invalid configuration leaves the running settings intact and displays an error.
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

### Terminal

- Local shells and arbitrary commands with configurable working directory and
  environment.
- More complete grapheme handling, font fallback, and IME.
- True color, text decorations, cursor styles, hyperlinks, clipboard support,
  search, selection, and configurable scrollback.
- More terminal mouse protocols, configurable keybindings, and shell
  integration.
- An Alacritty-based terminal engine initially, with the option to evaluate
  `libghostty-vt` after its public API matures.

### Desktop clients

- Native macOS, Linux, and Windows applications built with GPUI.
- Horizontal or vertical tab bars.
- Tabs containing arbitrary horizontal and vertical pane splits.
- Native fullscreen on supported platforms.
- Borderless non-native fullscreen on macOS without creating a separate
  Mission Control space.
- Multiple windows, drag-and-drop tab management, notifications, and restored
  window layouts.
- Accessibility and platform-native input behavior.

### Multiplexer

- A local server that owns terminal processes independently of client windows.
- Named sessions containing tabs and split-pane layouts.
- Detach and reattach without terminating terminal processes while the server
  remains alive.
- Multiple simultaneous clients with explicit terminal-size policy.
- A text-based client that can run inside another terminal.
- A command-line client for session and pane automation.
- Versioned local IPC, followed by optional authenticated remote attachment if
  the local design proves useful.

### State and recovery

- Persisted configuration, session metadata, tab order, and pane layouts.
- Scrollback and presentation-state recovery where it can be made reliable.
- Clear distinction between restoring saved presentation and preserving a live
  child process. A server crash is allowed to terminate its PTYs.

## Terminology

- **Session**: a named, server-owned collection of tabs.
- **Tab**: an ordered container with one pane layout.
- **Pane**: a leaf in a tab's split tree.
- **Terminal**: a PTY, child process, terminal-emulator state, and scrollback.
- **Client**: a desktop, TUI, or command-line connection to the runtime.
- **Client view**: client-local focus, active tab, viewport, and scroll
  position.

Tab order and pane layout belong to the session. Focus, the selected tab, and
viewport position belong to each client. Tab-bar orientation is a client
preference, not session state.

## Licensing

Huterm intends to license its own source under the MIT license. The planned
initial dependencies include MIT and Apache-2.0 software. Distributed builds
will retain the required third-party license notices and use an automated
license allowlist to prevent accidental GPL or AGPL dependencies.

Huterm-owned source is available under the [MIT license](LICENSE). The committed
dependency policy audits the full resolved graph and rejects GPL, AGPL, unknown
registries, Git dependencies, and wildcard Cargo requirements.
