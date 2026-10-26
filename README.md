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

- **Terminals:** 24-bit color, runtime-owned scrollback, selection, copy and
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
- **Presentation:** native and non-native fullscreen, and quake windows summoned
  by global shortcuts on macOS and native X11.
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
- Configurable scrollback limits, shell integration, desktop notifications,
  inline graphics, and window transparency.

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
agree, it checks every 24 hours. The update window lists the release notes for every
version between the installed one and the update.

To manage scheduled checks from the configuration file instead, set either key
in the `[updates]` table:

```toml
[updates]
automatic_checks = true
check_interval_hours = 24
```

An explicit `true` or `false` turns scheduled checks on or off without
Sparkle's prompt. Setting only the interval does not give consent. Leave a key
out to keep the value Sparkle has stored. Development builds, packages built
with `mise run package:macos`, and Linux builds do not include Sparkle and do
not contact the update feed.

### Linux

Each release has two packages for x86_64 and for aarch64:

- `Huterm-<version>-Linux-<arch>.AppImage` runs directly. Make it executable
  first. If FUSE is unavailable, run it with `--appimage-extract-and-run`.
- `Huterm-<version>-Linux-<arch>.tar.gz` holds the same payload for manual
  installation: `bin/huterm`, a desktop entry, AppStream metadata, an icon,
  and license notices. It has no AppImage runtime.

The packages need glibc 2.35 or newer (Ubuntu 22.04 or later), X11 or
XWayland, and a working Vulkan driver. GNOME on Wayland runs Huterm through
XWayland. Linux builds do not update themselves.

## Configuration

Huterm reads `$HUTERM_CONFIG_FILE` when it is set. Otherwise it reads
`$XDG_CONFIG_HOME/huterm/config.toml`, falling back to
`~/.config/huterm/config.toml`, on both macOS and Linux. The **Settings**
command (`open_settings`) creates a documented default file if none exists and
opens it in the system editor.

### Schema

Each release publishes JSON Schemas for the configuration and theme files. Add
this header to `config.toml` to get completion and validation in Taplo or the
Even Better TOML editor extension. New default files include it:

```toml
#:schema https://github.com/jimeh/huterm/releases/latest/download/huterm.schema.json
```

`latest/download` follows the newest release and may describe settings that an
older installation lacks. To match an installed version, use
`https://github.com/jimeh/huterm/releases/download/vX.Y.Z/huterm.schema.json`.
During development, point the directive at the absolute path of
`schemas/huterm.schema.json` in your checkout.

The schema validates settings and each command's keybinding arguments. For
example, `select_tab` accepts an optional integer `args.index` from 1 through 9,
while `copy` rejects that argument. Quake profiles use the same schema,
including geometry and animation limits. Global shortcuts allow only the three
quake commands and accept a text `profile` argument. Taplo completes command
names and reports bad arguments for the selected command. Completing a
command's arguments depends on the editor; Taplo 0.10.0 does not offer it.

Huterm checks the rest at runtime: keystroke and `when` syntax, platform
restrictions, theme files, theme inheritance cycles, quake profile names,
display availability, shortcut conflicts, and global shortcut registration.

### Reload

Choose **Reload Configuration** from the Huterm application menu on macOS or
the window menu, or press Cmd+Shift+, on macOS or Ctrl+Shift+, on Linux with a
US keyboard layout. GPUI represents these
chords as Cmd+< and Ctrl+<; on other layouts use the corresponding less-than
chord.

Reload rereads the configuration and the selected theme chain. It applies
fonts, padding, colors, and most other settings to every open window without
restarting shells or losing scrollback. Windows keep their size; visible grids
are recalculated, and hidden grids resize when selected. Where a setting
reloads differently, its section below says so.

An invalid, unreadable, or missing file leaves the running settings in place
and shows an error. To return to the defaults, empty the file and reload it.
Reload is manual: Huterm does not watch the file.

## Windows, sessions, and tabs

Each window owns a private session with one workspace of ordered tabs. Each tab
holds one pane with its own terminal and shell. Hidden tabs keep processing
output without building snapshots or painting.

### Shells

On macOS, Huterm launches `$SHELL -l` in the home directory, as a Finder launch
would. The packaged app also sets `LANG=en_US.UTF-8` when it inherits no locale
variable. On Linux, Huterm launches `$SHELL` in its own working directory.
Without `$SHELL`, it falls back to `/bin/zsh` on macOS and `/bin/sh` on Linux.

New tabs start in the current tab's directory when that directory is local and
still accessible. Remote, malformed, deleted, or inaccessible paths use the
normal launch directory instead:

```toml
[terminal]
new_tab_directory = "inherit" # inherit or default
```

A tab closes without confirmation when its shell exits. Set
`close_on_exit = false` in `[terminal]` to keep exited tabs for their history.
An exited tab allows selection, copy, scrolling, links, and resizing, but sends
no input. Reloading this setting affects only later exits.

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

### Tab bar

Tabs can sit at the top, bottom, left, or right of the window, or in the title
bar. The title bar is the macOS default; Linux defaults to the top. These are
the `[window]` and `[tabs]` settings with their defaults:

```toml
#:schema https://github.com/jimeh/huterm/releases/latest/download/huterm.schema.json

[window]
padding_x = 4.0
padding_y = 4.0
padding_balance = false
macos_fullscreen_mode = "non_native" # native or non_native; ignored on Linux
menu_button = true # the window menu button in the title bar or tab bar
shortcut_hints = false # key hints on the scroll pill and dialog buttons

[tabs]
label = "smart" # smart, title, process, directory, or process_and_directory
directory = "name" # name, path, or short
# position = "titlebar" # or top, bottom, left, right; Linux default: "top"
always_show = false
auto_hide_in_fullscreen = false
style = "pill" # pill or strip
pill_accent = false # accent bar inside the active pill
close_button = "active" # active, hover, or always
notch = "left" # fullscreen top bar beside a display notch: off, left, or right
width = "fit" # fit or fill
min_width = 96.0 # logical points; fit only
max_width = 240.0 # logical points; fit only
```

Padding accepts 0 to 256 points on each side. When the window width does not
fit whole columns, the leftover space stays on the right; set
`padding_balance = true` to split it between left and right. Leftover vertical
space always stays at the bottom.

`style` selects Pill or Strip tabs for every position. Left and right Strip
tabs put the accent line on the window edge; left and right Pill tabs are
rounded rows inset in the column. `pill_accent = true` draws an accent bar
inside the active pill. `close_button` shows close buttons on the active tab
and hovered tabs (`active`), on hovered tabs only (`hover`), or on every tab
(`always`). Themes color the tab bar through their tab chrome keys.

`width = "fit"` sizes top and bottom tabs to their titles between `min_width`
and `max_width`, each from 48 to 600 points. `width = "fill"` shares the bar
equally, shrinking tabs to 120 points before the bar scrolls. Left and right
tabs have a fixed height in a sidebar; drag its inner edge to resize it between
140 and 400 points, capped at half the window width. Each window keeps its own
sidebar width.

Drag a tab to reorder it within its window. The preview stays in the tab bar
even when the pointer leaves the window, releasing commits the position, and
Escape cancels. Drag near the end of the bar to scroll toward hidden tabs.
Scroll the bar with a trackpad or mouse wheel. Horizontal bars show arrows
where more tabs remain and a draggable position indicator along their bottom
edge. Vertical bars show an overlay scrollbar that widens on hover. The
new-tab button stays visible and follows the last tab in a vertical bar.

With one tab the bar stays hidden unless `always_show = true`.
`auto_hide_in_fullscreen = true` hides the bar in fullscreen until the pointer
reaches its edge, even with several tabs. The bar then slides over the
terminal without changing the grid and hides again when the pointer leaves.
Dragging a tab or resizing the sidebar keeps it open, and switching, creating,
or closing tabs reveals it for a second. A top bar beside a display notch has
nowhere to hide and stays in place.

In non-native fullscreen on a notched display, `notch = "left"` or `"right"`
moves a top bar beside the camera housing and gives its height back to the
terminal. `"off"` keeps the bar below the notch. Elsewhere the setting is
ignored.

Tabs show a small yellow dot before their title while a program holds the
foreground; exit, error, and bell indicators replace it. The window title is
`<active tab title> — Huterm`, so window switchers show the active tab. On
macOS the title bar shows the active tab's title in place of the application
name.

### Title bar tabs

`position = "titlebar"` merges the tabs into the window's title bar. On macOS
the 32-point title strip becomes the tab row. It starts after the traffic
lights, holds the tabs and `+`, keeps its trailing space draggable with the
usual double-click action, and ends with the menu button. The terminal gains
the height a top bar would take. With one tab and `always_show = false`, the
tabs stay hidden and the strip shows only the active tab's title.

On Linux, Huterm asks for client-side decorations and draws the whole row: the
tabs and `+`, draggable empty space, the menu button, and minimize, maximize,
and close controls. The controls follow the desktop's button layout, as GTK
applications do. GNOME's `button-layout` setting, for example through GNOME
Tweaks, or KDE's GTK settings choose which buttons appear and at which end, and
a change applies to open windows immediately. Huterm reads the layout from the
XSettings `Gtk/DecorationLayout` value and draws minimize, maximize, and close
on the right when the desktop publishes none.

Dragging the empty space moves the window, double-clicking it maximizes, and
right-clicking opens the window manager's menu. The close control takes the
same path as the `close_window` command. A windowed, untiled window also gets
a thin invisible resize border, a shadow, and rounded top corners, which go
away while it is maximized, tiled, or fullscreen.

Client-side decorations need a window manager that advertises
`_GTK_FRAME_EXTENTS`, such as GNOME's Mutter or KDE's KWin. Other window
managers, such as Openbox or i3, keep their own title bar, and `titlebar`
behaves as `top`. Fullscreen windows on either platform and quake windows have
no title bar, so their tabs use a top bar.

### Tab labels and directories

Huterm tracks each terminal's foreground program and working directory. The
directory comes from the foreground program, or from the shell when another
user owns that program. A bounded OSC 7 report overrides it while the program
that sent the report holds the foreground: a shell's report applies at its
prompt, and a remote shell's report under `ssh` stops applying when `ssh`
exits. Huterm checks after input, after output that follows a pause, after a
title change, and once a second while a program runs. Idle shells cost
nothing.

`label` chooses what each tab shows:

- `smart` (the default) shows a running program's own title, or its name when
  it has set none, and the directory at an idle shell. A program's title stops
  applying when the program exits.
- `title` shows the title set by the shell or program.
- `process` names the foreground program, or the script it runs.
- `directory` shows the working directory.
- `process_and_directory` combines both when available.

Each mode falls back to the terminal title and the launched program. A custom
tab name always wins. The palette, drag previews, and close prompts use the
same label as the tab bar.

`directory = "name"` (the default) shows the last path component, `"path"` the
full path, and `"short"` the path with parent components shortened to one
character, as in `~/P/huterm`. The local home directory reads as `~`; remote
paths stay absolute.

### Menus

`menu_button` shows a three-line button that opens the window menu: the command
palette, tab and window commands, Copy and Paste, fullscreen, settings, reload,
waiting notices, About, and Quit, each with its current shortcut. On macOS the
button sits at the right end of the title bar. Without a title bar, in
fullscreen or on Linux without client-side decorations, it joins the tab bar:
at the far right of a top or bottom bar, or beside `+` in a left or right
column. It hides with an auto-hidden bar. A yellow dot marks waiting notices.
`menu_button = false` removes the button; `open_menu` and every menu item stay
available through the palette and shortcuts. Right-clicking empty tab-bar
space also opens the window menu, except in Huterm's own Linux title row,
where it opens the window manager's menu.

Menus and the command palette show each command's shortcut: in symbols on macOS
(`⇧⌘P`) and in words on Linux (`Ctrl+Shift+P`), with Return, Tab, Backspace,
and the arrows drawn as icons on both. The scroll pill, the close
confirmation's buttons, and the About panel show key hints only with
`shortcut_hints = true`.

Right-clicking any tab opens its context menu without activating it, and
outlines the tab while the menu is open. The menu has:

- **Rename Tab…**
- **Copy Directory Path**, disabled with "Directory unknown" until the tab
  reports a working directory. It copies the local or remote path.
- **Close Tab**, with its shortcut shown only on the active tab.
- **Close Other Tabs**, disabled when there is one tab.
- **Close Tabs to the Right**, worded **Close Tabs Below** in a left or right
  column, disabled on the last tab.

The tab menu does not open during a tab drag or while a close confirmation is
showing. It uses the same keys, dismissal, and focus return as the window menu.

**About Huterm** (`about`) opens a panel with the version, identifier,
platform, terminal engine, and build profile, plus the short source revision
when the build embedded one. On Linux the platform line includes the display
backend. **Copy Details** copies those lines as text. Close, Enter, or Escape
closes the panel, and terminal input is blocked while it shows.

Errors and warnings appear as notices in the bottom-right corner of the
terminal. Configuration and keybinding diagnostics stay until a reload fixes
them. Command and terminal failures expire after six seconds unless hovered or
focused.

### Fullscreen

Cmd-Enter on macOS and F11 on both platforms toggle the configured fullscreen
mode. macOS defaults to non-native fullscreen in the current Space, which hides
the Dock and menu bar and restores the window's bounds on exit. Minimize and
Zoom are unavailable while a window is in non-native fullscreen. Set
`macos_fullscreen_mode = "native"` to use a separate Space instead. The green
window button always uses native fullscreen.

Every fullscreen command exits whichever mode is active. Rapid commands toggle
the desired state and wait for each transition to finish. Reload changes later
toggles without changing an active mode. Linux always uses native X11
fullscreen and reports an error if the window manager ignores the request; the
explicit non-native command is unavailable there.

### Quake windows

Quake windows are terminal windows summoned and hidden by profile name. Global
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

The `default` profile exists without configuration. Add named tables, such as
`[quake.profiles.logs]`, for independent windows. `show_quake`, `hide_quake`,
and `toggle_quake` accept an optional `profile` argument and also work in
ordinary `[[keybinding]]` entries. A hidden window keeps its tabs, shells,
scrollback, and selection. Toggling a visible but unfocused window raises it.
Toggling the focused window hides it and returns focus to the previous
application. Auto-hide does not take focus back from an application you
switched to. On macOS, launcher panels that take keyboard focus without
activating their application, such as 1Password Quick Access or Alfred, do not
hide the quake window.

`toggle_fullscreen` switches a quake window between quake and regular framed
presentation. Regular presentation suspends auto-hide and keeps your placement.
`toggle_native_fullscreen` and `toggle_non_native_fullscreen` are unavailable
for quake windows. Removing a named profile on reload turns its window into an
ordinary visible window without closing jobs. Closing a quake window's last tab
ends the association, so the next summon starts a new shell. Registered
shortcuts keep Huterm running when all windows are closed, and Quit still
assesses and closes every visible or hidden terminal.

Partial profiles are frameless and sized within the display's work area, with
a usable minimum. Tabs follow the configured single-tab and fullscreen
visibility settings. `position = "center"` centers both axes; other positions
anchor to that side and center the other axis.

`animation = "auto"` fades fullscreen profiles and fades and slides partial
profiles from their side; centered profiles only fade. `slide` follows the
position, using the top for centered profiles. Other values are `none`,
`fade`, `slide_top`, `slide_bottom`, `slide_left`, `slide_right`, and the
matching `fade_slide_*` values. Fullscreen quake windows use the current Space
on macOS and EWMH fullscreen on X11. Without an X11 compositor, fades are
disabled with a diagnostic and slides remain. A failed native transition
restores an opaque regular window and reports the error.

A quake window stays on its first display until it closes or moves in regular
presentation. A missing display falls back to the primary display. Explicit
IDs use CoreGraphics display UUIDs on macOS and RandR monitor names on X11.

Global bindings use a single keystroke, no `when`, and only the three quake
commands. They must not conflict with the first keystroke of a local binding.
Huterm reports unsupported keys and conflicts with other applications' grabs;
a rejected reload keeps the previous configuration and shortcuts. Global
shortcuts work on macOS and native X11 sessions, but not on Wayland or
XWayland.

## Terminals

### Fonts and themes

```toml
[font]
family = "Menlo" # defaults to Menlo on macOS and monospace on Linux
size = 14.0      # 6 through 96
```

Select a built-in theme and optionally override individual colors:

```toml
[theme]
name = "catppuccin-mocha"
background = "#181825"
ansi_red = "#f38ba8"
ansi_bright_red = "#eba0ac"
```

The built-in themes are `huterm-dark` (the default), `tokyo-night`,
`tokyo-night-storm`, `tokyo-night-moon`, `tokyo-night-day`,
`catppuccin-latte`, `catppuccin-frappe`, `catppuccin-macchiato`,
`catppuccin-mocha`, `dracula`, `nord`, `one-dark-pro`, `tomorrow-night`, and
`tango-with-monokai`. They are embedded in the application.

Define a named theme in your config:

```toml
[theme]
name = "my-mocha"

[themes.my-mocha]
extends = "catppuccin-mocha"
background = "#181825"
```

Or put it in `themes/my-mocha.toml` next to your config file:

```toml
#:schema https://github.com/jimeh/huterm/releases/latest/download/huterm-theme.schema.json

[theme]
extends = "catppuccin-mocha"
background = "#181825"
```

Theme names use letters, digits, hyphens, and underscores, without a file
extension. Lookup checks inline definitions, then theme files, then built-ins,
and the first match wins; definitions with the same name do not merge. Use a
distinct name when extending a built-in. A definition without `extends` starts
from Huterm Dark's palette, and any tab chrome key it leaves unset derives from
its resulting colors rather than from Huterm Dark's hand-picked set. An empty
`[theme]` is Huterm Dark. Overrides in the config's `[theme]` table apply last.

All color keys are flat:

- Terminal colors: `foreground`, `background`, `cursor`, `selection` (the
  selection background), optional `selection_foreground`, the eight
  `ansi_black` through `ansi_white` keys (`ansi_red`, `ansi_green`,
  `ansi_yellow`, `ansi_blue`, `ansi_magenta`, `ansi_cyan`), and the matching
  `ansi_bright_*` keys.
- Window chrome: `tab_bar_background`, `tab_active_background`,
  `tab_foreground`, `tab_inactive_foreground`, `tab_border`, and `tab_accent`,
  plus the overlays `tab_hover_background`, `scrollbar_thumb`, and
  `scrollbar_track`.

Colors use `#rrggbb`; the three overlay keys also accept `#rrggbbaa`. The
legacy 16-color `ansi = [...]` array still works, and individual ANSI keys
override its entries. Unspecified keys inherit. Without
`selection_foreground`, selected text keeps its original colors. Labels and
scroll controls derive their colors from the theme. Reload keeps colors that
terminal applications set explicitly.

See [theme sources and licenses](crates/huterm-gpui/themes/README.md) for
attribution and palette import details.

### Mouse, selection, and scrolling

Applications can request button reports with mode 1000, held-button motion
with 1002, or all motion with 1003. Huterm reports left, middle, and right
buttons and vertical and horizontal wheels, in legacy, UTF-8 (1005), or SGR
(1006) encoding, with Control and Alt/Option modifiers. Applications that do
not enable reporting receive no mouse input.

Hold Shift when starting a selection or scrolling to use local selection and
scrollback while an application has the mouse. The scrollbar always stays
local. Mouse input also stays local while you view scrollback, including while
a scroll away from live output waits for its snapshot; return to the live view
to interact with the application again. A drag keeps its original owner until
release, even if Shift changes or the pointer leaves the grid.

Leaving the live view or losing focus ends application button gestures.
Coordinates for continuing gestures clamp to the grid edge. Fast wheel input to
an application is limited to 32 steps per platform event, and steps that do
not fit the input queue are discarded. Legacy reports clamp to column and row
223 and UTF-8 reports to 2015; SGR covers the full grid.

Right-clicking the terminal opens its context menu; on macOS, Control-click
does the same. **Open Link** and **Copy Link Address** lead the menu when the
pointer is over a link. **Copy** is disabled until text is selected, and
**Paste** stays enabled while the shell runs. **Select All** selects the
scrollback and screen, **Clear Scrollback** erases history and keeps the
screen, and **Reset Terminal** clears the screen, history, and leftover modes,
as RIS does, without touching running programs. **Scroll to Bottom** appears
while the view shows history. The menu also offers **Copy Directory Path**,
**Show in Finder** or **Open in File Manager** for a local working directory,
and **New Tab**, **Rename Tab…**, and **Close Tab**. `open_context_menu` opens
the menu at the text cursor from the keyboard.

When a program such as vim or tmux has the mouse, a plain right-click goes to
the program and Shift+right-click opens the menu. `terminal.right_click`
chooses another action: `"menu"` (the default), `"paste"`, `"copy_or_paste"`
(copy and clear a selection, otherwise paste), or `"ignore"`. Menus opened at
the pointer shift up or down to fit the window and scroll only when taller
than it.

Visual bells briefly flash the active terminal and mark an inactive tab until
you view it. Bells never select a tab, activate a window, or move focus.
Reloading `visual = false` clears existing bell marks:

```toml
[terminal.bell]
visual = true
```

### Links and file drops

Hold Cmd on macOS or Ctrl on Linux and click an underlined HTTP(S) link to open
it with the system handler. Hovering shows the destination in the terminal's
bottom-left corner. OSC 8 links use their explicit destination, and plain URLs
can cross soft-wrapped rows. When an application has the mouse, add Shift to
the modifier chord. Moving away, scrolling, changing the target, or pressing
Escape cancels a click.

```toml
[terminal]
links = true
link_modifiers = "cmd" # macOS default; Linux defaults to "ctrl"
```

Modifier names are `cmd`, `ctrl`, `alt`, and `shift`, joined by `-` in any
nonempty combination. macOS rejects Ctrl because Control-click opens the
context menu. Reload applies both settings to open tabs. Only HTTP(S)
destinations open, and oversized or unavailable lookups show no link. Ghostty's
OSC 8 parser accepts at most 2,046 bytes of URI and parameters combined and
discards longer sequences.

Drop files or folders onto a live terminal to paste their absolute paths in
order, escaped for bash and zsh and followed by a space. Huterm keeps symlink
spelling, sends one paste, and never adds Enter. Bracketed paste follows the
application's current mode. An invalid, non-UTF-8, control-containing, or
oversized path rejects the whole drop. Exited terminals and confirmation
dialogs do not accept drops.

### Clipboard writes from applications

Terminal applications may write UTF-8 text to the system clipboard with OSC 52.
Huterm accepts the empty selector or `c`, keeps empty text and embedded NUL
bytes, and ignores clipboard read requests. Both BEL and ST end an OSC 52
write. Huterm also accepts Ghostty's OSC 1337 `Copy` extension and writes
through Kitty's clipboard protocol (OSC 5522). Kitty writes take the first
`text/plain` representation whose charset, if given, is UTF-8, ignore other
MIME types, and follow the same policy. Kitty programs receive:

- `DONE` on success;
- `EPERM` when the policy denies the write;
- `ENOSYS` for other clipboards or requests without text;
- `EINVAL` for text that is not UTF-8;
- `EFBIG` when all representations together exceed 64 MiB;
- `EBUSY` while earlier writes still fill the delivery queues.

Huterm drops writes that do not fit its bounded delivery queues instead of
truncating them. The queues hold at most 8 writes or 64 MiB per terminal, and
32 writes or 128 MiB across the app. Ghostty caps encoded OSC capture at 8 MiB,
and decoded Kitty write transactions at the 64 MiB per-terminal queue size, the
minimum the Kitty protocol requires.

Writes are allowed by default. Deny them without affecting the Copy and Paste
commands:

```toml
[terminal]
clipboard_write = "deny" # "allow" or "deny"
```

Reload applies the policy to open terminals and cancels writes that have not
reached the clipboard yet. An invalid reload keeps the last valid policy. At
startup, an explicit `deny` survives a fallback caused by an unrelated settings
error, and an invalid clipboard policy stops startup.

A terminal can write to the clipboard while its tab is inactive or its window
is hidden, minimized, or unfocused, so background output may replace the
clipboard. Use `deny` if you do not want that. Dropped writes are never
retried or replayed.

With tmux, `set-clipboard external` allows tmux copy mode and `set-buffer -w`
writes while blocking OSC 52 from applications inside tmux. Use
`set-clipboard on` to let applications inside tmux reach the clipboard too.

### Option and Alt as Meta

On macOS, set this to use either Option key for Meta chords in Emacs, tmux, or
readline:

```toml
[terminal]
macos_option_as_alt = "both"
```

The default, `"off"`, uses the macOS keyboard layout for Option characters and
dead-key composition. With `"both"`, Option-r sends ESC followed by `r`, and
Option-Shift-r sends ESC followed by `R`. Supported Control combinations keep
the Control byte after the ESC prefix. Special keys, such as Option-arrow, keep
their Alt-modified escape sequences with either setting. Separate left and
right Option settings are not supported.

Linux always sends unbound Alt character chords as Meta. Huterm shortcuts take
precedence on both platforms, including Linux's default Alt-1 through Alt-9 tab
selection; unbind a shortcut to pass its chord to applications.

Reload applies the setting to later input in open tabs; queued input keeps its
original interpretation. Changing the setting or leaving a terminal cancels
unfinished composition. Paste content is unaffected.

### Terminal identity, tmux, and SSH

New shells use Huterm's private `xterm-huterm` terminfo entry when it is
available locally. Development tasks compile it into `target/terminfo`, and the
macOS and Linux packages carry the compiled entry. If Huterm cannot find the
entry, the default `auto` setting falls back to `xterm-256color`.

```toml
[terminal]
term = "auto" # Or force "xterm-huterm" / "xterm-256color" for new shells.
```

`xterm-huterm` inherits the standard 256-color capabilities and advertises
`Tc`, so terminfo consumers such as tmux keep 24-bit color. If you force
`xterm-huterm`, making the entry available is your responsibility. A missing
entry usually appears as an "unknown terminal type" error in curses
applications.

An SSH server may not have the entry. Use a compatible identity for one SSH
command:

```sh
TERM=xterm-256color ssh host
```

Or install the local definition into your account on a trusted remote host:

```sh
infocmp -x xterm-huterm | ssh host 'tic -x -'
```

The packages also carry the source definition, at
`Huterm.app/Contents/Resources/terminfo/xterm-huterm.terminfo` on macOS and
`share/huterm/terminfo/xterm-huterm.terminfo` in the Linux packages. Copy it to
a remote host and run `tic -x xterm-huterm.terminfo` there as an alternative.

Install the entry where the application runs. `sudo`, `su`, and container
launchers can strip `TERMINFO` or `TERMINFO_DIRS`, change `HOME`, or use a
different filesystem, so an entry in your `~/.terminfo` does not reach root,
another user, a container, or a remote host. Run `tic` as the target user or
inside the target container, then check with `infocmp -x xterm-huterm`. When
installing is not appropriate, set `TERM` for the single command:

```sh
sudo env TERM=xterm-256color command
```

Replace `command` with the program to run as root. Huterm does not wrap SSH,
install files remotely, or change Mosh's terminal capability handling.

### Color, size, and appearance queries

Huterm answers the queries shells and development tools commonly send:

- `OSC 4;index;?` for palette indexes 0 through 255.
- `OSC 10;?`, `OSC 11;?`, and `OSC 12;?` for foreground, background, and
  cursor colors. `OSC 104`, `OSC 110`, `OSC 111`, and `OSC 112` reset the
  matching overrides.
- `CSI 14 t`, `CSI 16 t`, and `CSI 18 t` for content pixels, cell pixels, and
  rows and columns.
- `CSI ? 996 n` for dark or light appearance.

Replies use the terminal's current state. A color an application sets stays an
override even when it equals the theme color, so a theme reload does not
replace it; resetting the override exposes the current theme value. Size
replies describe the character grid and physical cell size, excluding window
chrome and padding.

### Engine and refresh

Every terminal uses Ghostty, and new configuration files have no engine
setting. Older files may still set one:

```toml
[terminal]
engine = "alacritty" # Deprecated; still launches Ghostty and reports a warning.
```

`"ghostty"` is accepted without a warning. Other values are configuration
errors.

Each terminal publishes snapshots in step with the window's display frames.
When output has used a frame's snapshot, a pending scroll may take one more so
scrolling stays responsive. Set `refresh = "unlimited"` in `[terminal]` to
request a new snapshot as soon as the previous one completes. That can raise
CPU use without showing more frames. Reload applies the setting to open tabs.

## Commands and keybindings

Every menu item, shortcut, and palette entry runs a command from one catalog.
Each command has a stable ID, a scope, and named typed arguments.

### Command palette

Open the command palette with Cmd+Shift+P on macOS or Ctrl+Shift+P on Linux, or
choose **View > Open Command Palette**. Search is fuzzy across command titles,
IDs, and descriptions, so `tfs` finds Toggle Fullscreen. Results rank by match
quality, then this window's recent commands, then how often the app has run
each command, then catalog order. Unavailable commands stay visible with the
reason they are unavailable.

Enter takes the most likely action. It runs a command whose arguments have
defaults, or opens its first required argument. Tab opens the arguments even
when Enter would run the command, so you can change optional values. The input
line shows the command, chips for the arguments already entered, and the active
argument. Tab and quake profile arguments use fuzzy pickers. Rename commands
prefill the current custom name, selected, so typing replaces it; a blank name
restores the default title.

Click places the cursor, drag or Shift-click selects, and double-click selects
a word. Shift with any movement key extends the selection. In an argument, Tab
moves to the next one, Backspace in an empty field reopens the previous chip or
returns to search, and Escape returns to search. Escape from search or a click
outside the palette closes it. Palette input never reaches the terminal
beneath it.

A keybinding that omits a prompted argument, such as `select_tab` without an
index, opens its picker directly. Cancelling from search keeps the query for 15
seconds by default, and reopening the palette in that time restores it
selected:

```toml
[palette]
placement = "top"           # top or center
retain_query = true
retain_query_seconds = 15   # 0 through 3600
```

`placement = "center"` centers the palette at its full height, so the input
line stays put while results filter down.

### Keybindings

Add `[[keybinding]]` entries to extend or replace the platform defaults:

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

The fields are:

- `key` (required): the keystroke or chord.
- `command` (required): a command ID from the [table below](#commands).
- `args`: a table of named arguments. Arrays are rejected.
- `when`: an optional context predicate.
- `description`: an optional label for the palette; blank text is rejected.
  Without it, the binding is labelled with the command title and its
  arguments, such as `Select Tab (index: 3)`.

Unknown fields are rejected, as everywhere in the config.

`unbind` is a reserved command that removes every earlier binding for the key,
including defaults, regardless of `when`. It takes no `args` or `when`.
Unbinding a key with no binding does nothing.

The platform defaults apply first, then user entries in file order. A later
entry wins over an earlier one with the same key and `when`. Two user entries
with the same key and `when` produce a conflict notice after reload, and the
last one wins. Bindings for one key with different `when` predicates coexist,
and GPUI prefers the one whose predicate matches closest to the focused view.
Among equals, a user binding with `when` beats one without, and otherwise the
later entry wins. macOS menus show a user binding without `when` ahead of the
default for the same command.

A context identifier matches only the innermost context that contains it, and
a binding without `when` matches at the innermost depth. So `fullscreen` on its
own ranks below a default for the same key while a terminal has focus, and
never runs. To require a window context while a terminal has focus, use the
descendant form: `when = "fullscreen > Terminal"`. Negations look through every
ancestor, so `Terminal && !confirming` works as written. Alternatively,
`unbind` the key first.

Bound chords never reach the shell; unbound ones do. Unbind `shift-pageup` to
send it to a full-screen program. Bind `alt-r` to a command and the shell never
sees the key. A binding with `when` takes its key only while the predicate
matches, so `ctrl-c` bound to `copy` with `when = "selection"` still interrupts
the shell when nothing is selected. The leading keystrokes of a multi-key chord
are always reserved.

Reload compiles the whole file. A bad entry keeps the last working bindings
and shows the diagnostic with its entry number and key. At startup, a bad entry
falls back to the defaults and shows the same error.

`key` uses GPUI keystroke syntax: the modifiers `cmd`, `ctrl`, `alt`, `shift`,
and `fn` joined with hyphens before the key name, as in `cmd-shift-w`,
`ctrl-tab`, `f11`, or `shift-pageup`. A `+` between modifiers, as in `cmd+<`,
is rejected rather than parsed as an unmodified key; a trailing `+` is the plus
key. Separate the keystrokes of a multi-key chord with spaces: `ctrl-k ctrl-t`.
Write shifted punctuation as the resulting symbol, so Cmd+Shift+, is `cmd-<`.

`when` uses GPUI's predicate language: context identifiers combined with `&&`,
`||`, `!`, `==` and `!=` for `key == value` entries, and `>` to require a
descendant context (`Workspace > Terminal`). The contexts are:

| Context | Present when |
| --- | --- |
| `Terminal` | A terminal view has focus. |
| `selection` | That terminal has selected text. |
| `exited` | That terminal's root shell has exited. |
| `Workspace` | Always, on the window's root. |
| `confirming` | A close confirmation or the About panel is open. |
| `menu` | The window menu has keyboard focus. |
| `notices` | A notice has keyboard focus (after `focus_notices`). |
| `reordering` | A tab drag is in progress. |
| `fullscreen` | The window has finished entering native or non-native fullscreen. Pending entry alone does not match. |
| `palette` | The window owns an open command palette. |
| `Palette` | The command palette is open and has focus. |

### Commands

A command's scope says who runs it. Runtime commands run in the core against
the canonical session structure; their ID arguments cannot be written in the
config and are filled from the invoking window. Palette-scope commands imply
the `Palette` context, and dialog, menu, and notice commands imply
`confirming`, `menu`, and `notices`. User bindings for them need no `when`.

| Command | Scope | Arguments |
| --- | --- | --- |
| `show_quake` | Application | Optional `profile` name; defaults to `default`. |
| `hide_quake` | Application | Optional `profile` name; defaults to `default`. |
| `toggle_quake` | Application | Optional `profile` name; defaults to `default`. |
| `new_window` | Application | |
| `quit` | Application | |
| `hide` | Application | |
| `hide_others` | Application | |
| `show_all` | Application | |
| `reload_config` | Application | |
| `check_for_updates` | Application | macOS release builds only. |
| `open_settings` | Window | |
| `about` | Window | |
| `open_command_palette` | Window | |
| `new_tab` | Window | |
| `close_tab` | Window | Optional `tab`; defaults to the active tab. |
| `close_other_tabs` | Window | Optional `tab`; defaults to the active tab. |
| `close_tabs_after` | Window | Optional `tab`; defaults to the active tab. |
| `copy_tab_directory` | Window | Optional `tab`; defaults to the active tab. |
| `open_tab_directory` | Window | Optional `tab`; defaults to the active tab. Requires a local directory. |
| `close_window` | Window | |
| `next_tab` | Window | |
| `previous_tab` | Window | |
| `select_tab` | Window | `index` (1 to 9; 9 selects the last tab) or `tab`, not both. Without either, prompts for a tab. |
| `select_recent_tab` | Window | Activates the most recently used tab; repeat to toggle between two tabs. |
| `toggle_fullscreen` | Window | |
| `toggle_native_fullscreen` | Window | Enters native mode, or exits any active fullscreen mode. |
| `toggle_non_native_fullscreen` | Window | macOS only. Enters current-Space mode, or exits any active fullscreen mode. |
| `minimize` | Window | |
| `zoom` | Window | |
| `open_menu` | Window | |
| `open_context_menu` | Window | Opens the active terminal's context menu at its cursor. |
| `focus_notices` | Window | Focuses the newest notice; unavailable without notices. |
| `dismiss_all_notices` | Window | Dismisses every notice; unavailable without notices. |
| `copy` | Terminal | Requires a selection. |
| `paste` | Terminal | |
| `select_all` | Terminal | Selects the scrollback and screen. |
| `clear_scrollback` | Terminal | Erases the scrollback and keeps the screen. |
| `reset_terminal` | Terminal | Resets the screen, scrollback, and modes, as RIS does. |
| `scroll_page_up` | Terminal | |
| `scroll_page_down` | Terminal | |
| `scroll_to_bottom` | Terminal | |
| `rename_tab` | Runtime | `name`; `tab` defaults to the active tab. |
| `rename_workspace` | Runtime | `name`; `workspace` defaults to the window's workspace. |
| `rename_session` | Runtime | `name`; `session` defaults to the window's session. |
| `dialog_confirm` | Window | Bound only while `confirming`. |
| `dialog_cancel` | Window | Bound only while `confirming`. |
| `dialog_focus_next` | Window | Bound only while `confirming`. |
| `dialog_focus_previous` | Window | Bound only while `confirming`. |
| `menu_select_next` | Window | Bound only while `menu`. |
| `menu_select_previous` | Window | Bound only while `menu`. |
| `menu_select_first` | Window | Bound only while `menu`. |
| `menu_select_last` | Window | Bound only while `menu`. |
| `menu_select_right` | Window | Bound only while `menu`. |
| `menu_select_left` | Window | Bound only while `menu`. |
| `menu_confirm` | Window | Bound only while `menu`. |
| `menu_close` | Window | Bound only while `menu`. |
| `notice_next` | Window | Bound only while `notices`. |
| `notice_previous` | Window | Bound only while `notices`. |
| `notice_run_action` | Window | Bound only while `notices`. |
| `notice_dismiss` | Window | Bound only while `notices`. |
| `palette_select_next` | Palette | |
| `palette_select_previous` | Palette | |
| `palette_page_down` | Palette | |
| `palette_page_up` | Palette | |
| `palette_confirm` | Palette | |
| `palette_back` | Palette | |
| `palette_pop` | Palette | |
| `palette_expand` | Palette | |
| `palette_next_slot` | Palette | |
| `palette_previous_slot` | Palette | |
| `text_delete_backward` | Palette | |
| `text_delete_forward` | Palette | |
| `text_delete_word_backward` | Palette | |
| `text_delete_word_forward` | Palette | |
| `text_delete_line_start` | Palette | |
| `text_move_left` | Palette | Optional `select` extends the selection instead. |
| `text_move_right` | Palette | Optional `select`. |
| `text_move_word_left` | Palette | Optional `select`. |
| `text_move_word_right` | Palette | Optional `select`. |
| `text_line_start` | Palette | Optional `select`. |
| `text_line_end` | Palette | Optional `select`. |
| `text_select_all` | Palette | |
| `text_copy` | Palette | |
| `text_paste` | Palette | |

### Default bindings

Commands without a default binding are available through the palette, the
menus, or your own bindings.

#### Application, window, and terminal

| Command | macOS | Linux |
| --- | --- | --- |
| `new_window` | `cmd-n` | `ctrl-shift-n` |
| `open_command_palette` | `cmd-shift-p` | `ctrl-shift-p` |
| `new_tab` | `cmd-t` | `ctrl-shift-t` |
| `close_tab` | `cmd-w` | `ctrl-shift-w` |
| `close_window` | `cmd-shift-w` | `ctrl-shift-q` |
| `next_tab` / `previous_tab` | `ctrl-tab` / `ctrl-shift-tab` | `ctrl-tab` / `ctrl-shift-tab` |
| `select_tab` 1 to 9 | `cmd-1` to `cmd-9` | `alt-1` to `alt-9` |
| `copy` / `paste` | `cmd-c` / `cmd-v` | `ctrl-shift-c` / `ctrl-shift-v` |
| `select_all` | `cmd-a` | `ctrl-shift-a` |
| `clear_scrollback` | `cmd-k` | |
| `scroll_page_up` / `scroll_page_down` | `shift-pageup` / `shift-pagedown` | `shift-pageup` / `shift-pagedown` |
| `scroll_to_bottom` | `shift-end` | `shift-end` |
| `toggle_fullscreen` | `cmd-enter`, `f11` | `f11` |
| `reload_config` | `cmd-<` | `ctrl-<` |
| `open_settings` | `cmd-,` | `ctrl-,` |
| `quit` | `cmd-q` | |
| `minimize` | `cmd-m` | |
| `hide` / `hide_others` | `cmd-h` / `cmd-alt-h` | |

#### Palette

| Command | macOS | Linux |
| --- | --- | --- |
| `palette_select_next` | `down`, `ctrl-n` | `down`, `ctrl-n` |
| `palette_select_previous` | `up`, `ctrl-p` | `up`, `ctrl-p` |
| `palette_page_down` | `pagedown` | `pagedown` |
| `palette_page_up` | `pageup` | `pageup` |
| `palette_confirm` | `enter` | `enter` |
| `palette_back` | `escape` | `escape` |
| `palette_expand` | `tab` | `tab` |
| `palette_previous_slot` | `shift-tab` | `shift-tab` |
| `text_delete_backward` | `backspace` | `backspace` |
| `text_delete_forward` | `delete`, `ctrl-d` | `delete` |
| `text_delete_word_backward` | `alt-backspace` | `alt-backspace` |
| `text_delete_word_forward` | `alt-d` | `alt-d` |
| `text_delete_line_start` | `cmd-backspace` | |
| `text_move_left` / `text_move_right` | `left` / `right` | `left` / `right` |
| `text_move_word_left` / `text_move_word_right` | `alt-left`, `alt-b` / `alt-right`, `alt-f` | `alt-left`, `alt-b` / `alt-right`, `alt-f` |
| `text_line_start` / `text_line_end` | `home`, `ctrl-a`, `cmd-left` / `end`, `ctrl-e`, `cmd-right` | `home` / `end` |
| movement with `select = true` | `shift-` plus each movement key above | `shift-` plus each movement key above |
| `text_select_all` | `cmd-a` | `ctrl-a` |
| `text_copy` | `cmd-c` | `ctrl-c`, `ctrl-shift-c` |
| `text_paste` | `cmd-v` | `ctrl-v`, `ctrl-shift-v` |

#### Dialog

These bind only while a close confirmation or the About panel is showing
(`confirming`), so they take nothing from terminal input. The primary button
has focus when the dialog opens, and a repeated close shortcut never confirms.
In the About panel, `dialog_confirm` and `dialog_cancel` both close it and the
focus commands do nothing.

| Command | macOS | Linux |
| --- | --- | --- |
| `dialog_confirm` | `enter` | `enter` |
| `dialog_cancel` | `escape` | `escape` |
| `dialog_focus_next` | `tab`, `right` | `tab`, `right` |
| `dialog_focus_previous` | `shift-tab`, `left` | `shift-tab`, `left` |

#### Menu

These bind only while the window menu has focus (`menu`). `open_menu` has no
default binding. The menu button opens the menu with nothing selected, and
`open_menu` selects the first item. Typing a letter jumps to the next item
starting with it. Escape, Tab, running an item, or clicking elsewhere closes
the menu and returns focus to the terminal.

| Command | macOS | Linux |
| --- | --- | --- |
| `menu_select_next` | `down` | `down` |
| `menu_select_previous` | `up` | `up` |
| `menu_select_first` | `home` | `home` |
| `menu_select_last` | `end` | `end` |
| `menu_select_right` | `right` | `right` |
| `menu_select_left` | `left` | `left` |
| `menu_confirm` | `enter`, `space` | `enter`, `space` |
| `menu_close` | `escape`, `tab` | `escape`, `tab` |

#### Notices

These bind only while a notice has focus (`notices`), which `focus_notices`
gives to the newest notice. Dismissing the last focused notice returns focus to
the terminal.

| Command | macOS | Linux |
| --- | --- | --- |
| `notice_next` | `down` | `down` |
| `notice_previous` | `up` | `up` |
| `notice_run_action` | `enter` | `enter` |
| `notice_dismiss` | `escape`, `delete` | `escape`, `delete` |

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
  layouts, moving tabs between windows, shell integration, configurable
  scrollback, a settings UI, window transparency, and restoring sessions and
  history after a restart.
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
