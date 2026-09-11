# Huterm

Huterm is an experimental terminal emulator and multiplexer written in Rust.
It will start as a native desktop application built with GPUI, but the terminal
runtime will not belong to the desktop UI. Huterm will own its pseudoterminals,
terminal state, sessions, workspaces, tabs, and pane layouts so other clients
can attach to the same runtime later.

The first proof-of-concept implementation is under active validation. It has a
working PTY runtime, incremental terminal snapshots, and macOS/Linux GPUI clients.
Portable core tests, repository checks, Linux native tests, and the Linux Xvfb
smoke pass locally. CI enforces the same suite on Apple Silicon macOS and native
Linux x86_64/aarch64; green current-head CI is required for PR readiness, while
manual visual/input evidence remains before the milestone is complete.

## Installation

Releases provide a universal macOS app plus native Linux x86_64 and aarch64
builds. Each Linux architecture has an AppImage for direct launch and a neutral
binary tarball for manual installation. The tarball contains no AppImage runtime
or AppImage-only files.

Linux packages target glibc 2.35 or newer and require X11 or XWayland plus a
working Vulkan driver. Make an AppImage executable before launching it. If FUSE
is unavailable, use `--appimage-extract-and-run` or install the tarball instead.
Verify downloads against the release's `SHA256SUMS`.

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

Huterm runs on macOS Apple Silicon and Linux x86_64/aarch64. Each native window
has a private session and backing workspace with ordered tabs, one pane per tab,
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

Add this header to `config.toml` to associate its schema in Taplo or the
Even Better TOML editor extension. Newly created default configs include it:

```toml
#:schema https://github.com/jimeh/huterm/releases/latest/download/huterm.schema.json
```

The schema validates settings and command-specific keybinding arguments. For
example, `select_tab` requires an integer `args.index` from 1 through 9, while
`copy` rejects that argument. Huterm still checks keystroke and `when` syntax,
platform restrictions, theme files, and inheritance cycles at runtime.
Quake profiles share the same schema, including geometry and animation limits.
Global shortcuts offer only the three quake commands and accept a text `profile`
argument. Profile existence, display availability, shortcut conflicts, and OS
registration are checked at runtime.
Taplo completes command names and diagnoses arguments for the selected command.
Command-specific argument completion depends on the editor; Taplo 0.10.0 does
not currently offer it.

`latest/download` follows the newest published release and may describe settings
missing from an older installation. To match an installed version, use
`https://github.com/jimeh/huterm/releases/download/vX.Y.Z/huterm.schema.json`.
For development, point the directive at the absolute local path to
`schemas/huterm.schema.json`. Release URLs become available after the first
release containing these assets is published.

Terminal padding defaults to 4 logical points on each side. Add or adjust the
`[window]` section to change it:

```toml
#:schema https://github.com/jimeh/huterm/releases/latest/download/huterm.schema.json

[window]
padding_x = 4.0
padding_y = 4.0
padding_balance = false
tab_position = "top" # top, bottom, left, or right
always_show_tab_bar = false
auto_hide_tab_bar_in_fullscreen = false
macos_fullscreen_mode = "non_native" # native or non_native; ignored on Linux
```

Set `padding_balance = true` to split leftover horizontal space evenly between
left and right when the window width does not fit whole columns. With it off,
the remainder stays on the right. Vertical remainder always stays at the
bottom. Padding accepts values from 0 to 256 points.

Cmd-Enter on macOS, and F11 on either platform, toggle the configured fullscreen
mode. macOS defaults to non-native fullscreen in the current Space. Set
`macos_fullscreen_mode = "native"` to create a separate macOS Space instead.
The green window button always uses AppKit's native fullscreen behavior.
Non-native mode auto-hides the Dock and menu bar and restores
the window's previous bounds on exit. Minimize and Zoom are unavailable while
that window owns non-native presentation state.

All fullscreen commands exit whichever mode is active. Rapid commands toggle
the desired state and wait for each platform transition to finish. Reload
changes future default toggles without changing an active mode. Linux always
uses native X11 fullscreen and reports an error if the window manager ignores
the request; the explicit non-native command is unavailable there.

### Links and file drops

Hold Cmd on macOS or Ctrl on Linux and click an underlined HTTP(S) link to open
it with the system handler. Hover shows the destination. OSC 8 labels use their
explicit destination; plain URLs can cross soft-wrapped rows. When a terminal
application captures the mouse, add Shift to the configured modifier chord.
Moving away, scrolling, changing the target, or pressing Escape cancels a click.
Retained exited history supports links too.

```toml
[terminal]
links = true
link_modifiers = "cmd" # macOS default; Linux defaults to "ctrl"
```

Modifier names are `cmd`, `ctrl`, `alt`, and `shift`, joined by `-` in any
nonempty combination. macOS rejects Ctrl because AppKit uses
Control-click for the context menu. Reload applies both settings to open tabs.
Only HTTP(S) destinations open. Oversized or unavailable lookups show no link.
Ghostty's pinned OSC 8 parser accepts at most 2,046 combined URI and parameter
bytes and discards larger sequences; Alacritty has a larger native limit.

Drop files or folders onto a live terminal to paste their absolute paths in
source order, escaped for bash/zsh and followed by a space. Huterm preserves
symlink spelling, sends one paste, and never adds Enter. Bracketed paste follows
the application's current mode. An invalid, non-UTF-8, control-containing, or
oversized path rejects the whole drop. Exited terminals and confirmation dialogs
do not accept file drops.

### Option and Alt as Meta

On macOS, set this to use either Option key for terminal Meta chords in Emacs,
tmux, or readline:

```toml
[terminal]
macos_option_as_alt = "both"
```

The default is `"off"`, which uses the selected macOS keyboard layout for
printable Option characters and dead-key composition. With `"both"`, Option-r
sends ESC followed by `r`, and Option-Shift-r sends ESC followed by `R`.
Supported Control combinations retain
the Control byte after the ESC prefix. Special keys, such as Option-arrow,
keep their existing Alt-modified escape sequences with either setting.
Left/right-only Option settings are not supported yet.

Linux always sends unbound Alt character chords as Meta. Huterm shortcuts take
precedence on both platforms, including Linux's default Alt-1 through Alt-9 tab
selection. Unbind a shortcut to pass that chord to terminal applications.

Reload applies the policy to subsequent input in existing tabs; queued input
keeps its original interpretation. Invalid reloads leave the working settings
in place. Changing the policy or leaving a terminal cancels unfinished text
composition. Paste content is unaffected by the Meta setting.

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
#:schema https://github.com/jimeh/huterm/releases/latest/download/huterm-theme.schema.json

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

### Keybindings

Quake windows are retained terminal windows summoned by profile name. Global
shortcuts are opt-in and work while another application has focus:

```toml
[quake.profiles.default]
position = "top"            # top, bottom, left, right, center
width = 1.0                 # fraction of work area, greater than 0 through 1
height = 0.5
fullscreen = false          # use the full display frame, overriding width/height
display = "active"          # active, pointer, primary, or id:<platform identifier>
hide_on_focus_loss = true
animation = "auto"
animation_ms = 150          # 0 through 1000; zero is immediate

[[global_keybinding]]
key = "ctrl-alt-t"
command = "toggle_quake"
# args = { profile = "default" }
```

The built-in `default` profile exists without configuration. Add named tables,
such as `[quake.profiles.logs]`, for independent windows. `show_quake`,
`hide_quake`, and `toggle_quake` accept an optional `profile` argument and also
work in ordinary `[[keybinding]]` entries. A hidden window keeps its tabs, shell,
scrollback, and selection. Toggling a visible but unfocused window raises it;
toggling the focused window hides it and returns focus to the previous external
application. Auto-hide does not take focus back from the application you chose.

`toggle_fullscreen` switches an associated window between quake and regular
framed presentation. Regular presentation suspends auto-hide and retains user
placement. Explicit `toggle_native_fullscreen` and `toggle_non_native_fullscreen`
commands report unavailable for associated windows. Removing a named profile on
reload converts its window to a visible ordinary window without closing jobs.
Closing its final tab clears the association; the next summon creates a new shell.
Registered shortcuts keep Huterm available when all windows are closed. Quit
still assesses and closes every visible or hidden terminal.

Partial profiles are frameless and use the work area. Tabs follow the configured
single-tab and fullscreen visibility settings. Their size is clamped to a usable
minimum without exceeding the work area. `position = "center"` centers both axes;
the other positions anchor to that side and center the perpendicular axis.
`auto` fades fullscreen profiles and fades/slides partial profiles from their
anchored position; centered profiles only fade. `slide` follows the position,
using the top for centered profiles. Other values are `none`, `fade`,
`slide_top`, `slide_bottom`,
`slide_left`, `slide_right`, and the corresponding `fade_slide_*` values.
Fullscreen quake uses the current Space on macOS and EWMH fullscreen on X11.
Without an X11 compositor, fades are disabled with a diagnostic; slides remain.
A failed native transition restores an opaque regular window and reports the error.

The initial display is retained until the window closes or moves in regular
presentation. Missing displays fall back to the primary display. Explicit IDs use
CoreGraphics display UUIDs on macOS and RandR monitor names on X11. Global
bindings use one stroke, no `when`, and only the three quake commands. They must
not conflict with a local binding's first stroke. Unsupported keys and OS grab
conflicts are reported; a rejected reload retains the previous configuration and
registered shortcuts. Native X11 sessions and macOS are supported; Wayland and
XWayland global shortcuts are unavailable.

Every shortcut runs a catalog command. Add `[[keybinding]]` entries to extend
or replace the platform defaults:

```toml
[[keybinding]]
key = "cmd-1"
command = "select_tab"
args = { index = 1 }
when = "Terminal && !confirming"
description = "Select the first tab"

[[keybinding]]
key = "shift-pageup"
command = "unbind"
```

Fields: `key` (required), `command` (required), `args` (a table of named
arguments; arrays are rejected), `when` (an optional context predicate), and
`description` (an optional label for the settings UI and palette; blank text
is rejected). Without `description`, the binding is labelled with the command
title and its arguments, such as `Select Tab (index: 3)`. Unknown fields are
rejected like the rest of the config.

`unbind` is a reserved command that removes every earlier binding for the key,
including defaults, regardless of `when`. It takes no `args` or `when`, and
unbinding a key that has no binding does nothing.

Precedence: the platform defaults apply first, then user entries in file
order. A later entry wins over an earlier one for the same key and `when`;
two user entries with the same key and `when` produce a conflict notice in the
window status after reload, and the last one wins. Different `when` predicates
for one key coexist; GPUI then prefers the binding whose predicate matches
closest to the focused view. Among equals, a user binding with `when` beats
one without, and otherwise the later entry wins. macOS menus display a user
binding without `when` ahead of the default for the same command.

A context identifier matches only the innermost context that contains it, and
a binding with no `when` matches at the innermost depth. So `fullscreen` on its
own ranks below a default for the same key while a terminal is focused, and
never runs. To require a window context while a terminal is focused, use the
descendant form: `when = "fullscreen > Terminal"`. Negations look through every
ancestor, so `Terminal && !confirming` works as written. Alternatively `unbind`
the key first.

Bound chords never reach the shell; unbound ones do. Unbind `shift-pageup` to
send it to a full-screen program, or bind `alt-r` to a command and the shell
never sees the key or its typed character. A binding with `when` consumes its
key only while the predicate matches, so `ctrl-c` bound to `copy` with
`when = "selection"` still interrupts the shell when nothing is selected. The
prefix keystrokes of a multi-key chord are always reserved. Reload compiles
the whole file: a
bad entry keeps the last working bindings and shows the diagnostic with its
entry number and key. At startup a bad entry falls back to the defaults with
the same error shown in the status line.

`key` uses GPUI keystroke syntax: modifiers `cmd`, `ctrl`, `alt`, `shift`, and
`fn` joined with hyphens before the key name (`cmd-shift-w`, `ctrl-tab`,
`f11`, `shift-pageup`). A `+` between modifiers, such as `cmd+<`, is rejected
rather than silently parsed as an unmodified key; a trailing `+` is the plus
key. Separate the keystrokes of a multi-key chord with spaces: `ctrl-k ctrl-t`.
Shifted punctuation is written as the resulting symbol, so Cmd+Shift+, is
`cmd-<`.

`when` uses GPUI's predicate language: context identifiers combined with `&&`,
`||`, `!`, `==` and `!=` for `key == value` entries, and `>` to require a
descendant context (`Workspace > Terminal`). Available contexts:

| Context | Present when |
| --- | --- |
| `Terminal` | A terminal view has focus. |
| `selection` | That terminal has selected text. |
| `exited` | That terminal's root shell has exited. |
| `Workspace` | Always, on the window's root. |
| `confirming` | A close confirmation is open. |
| `reordering` | A tab drag is in progress. |
| `fullscreen` | The window has completed entry into native or non-native fullscreen. Pending entry alone does not match. |

`Palette` is reserved for the future command palette.

Commands, their scope, and arguments:

| Command | Scope | Arguments |
| --- | --- | --- |
| `new_window` | Application | |
| `quit` | Application | |
| `hide` | Application | |
| `hide_others` | Application | |
| `show_all` | Application | |
| `reload_config` | Application | |
| `open_settings` | Window | |
| `about` | Window | |
| `new_tab` | Window | |
| `close_tab` | Window | |
| `close_window` | Window | |
| `next_tab` | Window | |
| `previous_tab` | Window | |
| `select_tab` | Window | `index` (1 to 9; 9 selects the last tab) |
| `show_quake`, `hide_quake`, `toggle_quake` | Application | Optional `profile` string; defaults to `default`. |
| `toggle_fullscreen` | Window | |
| `toggle_native_fullscreen` | Window | Enter native mode, or exit any active fullscreen mode. |
| `toggle_non_native_fullscreen` | Window | macOS only. Enter current-Space mode, or exit any active fullscreen mode. |
| `minimize` | Window | |
| `zoom` | Window | |
| `copy` | Terminal | |
| `paste` | Terminal | |
| `scroll_page_up` | Terminal | |
| `scroll_page_down` | Terminal | |
| `scroll_to_bottom` | Terminal | |
| `rename_tab` | Runtime | `name`; `tab` defaults to the active tab |
| `rename_workspace` | Runtime | `name`; `workspace` defaults to the window's workspace |
| `rename_session` | Runtime | `name`; `session` defaults to the window's session |

Runtime commands execute in the core against canonical structure; the ID
arguments cannot be written in config and are filled from the invoking window.

Default bindings differ per platform:

| Command | macOS | Linux |
| --- | --- | --- |
| `new_window` | `cmd-n` | `ctrl-shift-n` |
| `new_tab` | `cmd-t` | `ctrl-shift-t` |
| `close_tab` | `cmd-w` | `ctrl-shift-w` |
| `close_window` | `cmd-shift-w` | `ctrl-shift-q` |
| `next_tab` / `previous_tab` | `ctrl-tab` / `ctrl-shift-tab` | `ctrl-tab` / `ctrl-shift-tab` |
| `select_tab` 1 to 9 | `cmd-1` to `cmd-9` | `alt-1` to `alt-9` |
| `copy` / `paste` | `cmd-c` / `cmd-v` | `ctrl-shift-c` / `ctrl-shift-v` |
| `scroll_page_up` / `scroll_page_down` | `shift-pageup` / `shift-pagedown` | `shift-pageup` / `shift-pagedown` |
| `scroll_to_bottom` | `shift-end` | `shift-end` |
| `toggle_fullscreen` | `cmd-enter`, `f11` | `f11` |
| `reload_config` | `cmd-<` | `ctrl-<` |
| `open_settings` | `cmd-,` | |
| `quit` | `cmd-q` | |
| `minimize` | `cmd-m` | |
| `hide` / `hide_others` | `cmd-h` / `cmd-alt-h` | |

Commands without a default binding are available through menus or config.

On macOS, build a universal application bundle with:

```sh
mise run package:macos
```

The package task installs both Rust targets, builds both terminal engines for
arm64 and x86_64, and combines the executables into
`target/release/bundle/Huterm.app`. It also checks the packaged macOS privacy
descriptions and release entitlements, but does not sign or notarize local
packages. Intel hardware validation remains pending.

Use `mise tasks` to discover all commands. `mise run check` is the fast local
gate, while `mise run verify` also runs tests, the dependency-license policy,
and GitHub Actions checks. See the
[development guide](docs/agents/development.md) for platform prerequisites,
limits, and the validation ladder.

Release Please creates draft GitHub releases. The release workflow signs,
notarizes, staples, verifies, and publishes the universal app only after its
uploaded ZIP and checksum match the local artifacts. See the
[release guide](docs/agents/releases.md) for the required repository variables,
secrets, recovery workflow, and remaining manual checks.

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
- More terminal mouse protocols and shell integration.
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
