# Configuration

Huterm reads one TOML file. This page describes each setting next to the
behavior it controls, along with related terminal behavior that has no setting,
such as mouse reporting and terminfo. Keybindings and the command palette have
their own page: [Commands and keybindings](keybindings.md).

## Configuration file

Huterm reads `$HUTERM_CONFIG_FILE` when it is set. Otherwise it reads
`$XDG_CONFIG_HOME/huterm/config.toml`, falling back to
`~/.config/huterm/config.toml`, on both macOS and Linux. The **Open Config
File** command (`open_config_file`) creates a documented default file if none
exists and opens it in the system editor.

## Settings window

**Open Settings** (`open_settings`, Cmd+, on macOS and Ctrl+, on Linux) opens
the Settings window, or brings it forward when it is open. It edits the same
file: each change saves at once and changes only the lines that setting owns,
so comments, ordering, and formatting stay as you wrote them. A dot marks each
setting the file sets, and its Reset button, a counterclockwise arrow, removes
it and returns the setting to its default. A setting that does not apply at
once says when it does, such as "Applies to new terminals". Search finds
settings in every section, and settings marked Advanced appear after you turn
on Show advanced settings in their section.

Saving applies the change even when `watch_config` is off. When the file does
not parse or has an invalid setting, Settings shows the error, keeps the
running settings, and is read-only until the file is fixed. When another
editor changed the same setting since Settings read it, the row keeps your
value and offers **Use mine**, which saves it over the file's, or **Use
file**, which keeps the file's. A field you are still typing in keeps your
text when the file reloads, and Escape puts its value back. While watching
is off, Settings notices a changed file when its window comes forward and
offers Reload. Closing Settings saves the field you were typing in and
waits for pending saves.

Theme colors and `[themes]` are edited in the file for now. In the window,
Tab moves between controls, Space or Return toggles a switch or opens a
select, the arrow keys step a segmented control and move between the link
modifier chips but leave a switch alone, Cmd+Backspace on macOS or
Ctrl+Backspace on Linux resets the focused setting, and Return in the search
field moves to the first match. Number fields keep to their setting's range:
Up and Down and the + and − buttons step within it, Page Up and Page Down
move a tenth of the range (ten steps when either end is open), and a typed
number outside it saves as the nearest end. The first step from a value off
the step grid snaps it onto the grid, so 13.5 steps up to 14, and the first
step from an empty field goes to the setting's minimum.
A Quake profile's width and height are sliders from 5% to 100% in 5% steps:
the arrow keys step them, Page Up and Page Down move 10%, and Home and End
go to 5% and 100%. A value the file sets off that grid shows as it is, and
the first step moves it onto the grid.

## Schema

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

## Reload

Huterm watches `config.toml` and reloads it shortly after it changes on disk,
including when the file is a symlink and when an editor saves by renaming. It
also watches the theme files the selected theme reads, so editing a theme in
`themes/`, or a theme it extends, reloads too. A successful automatic reload
shows no notice.

```toml
[general]
# Reload this file automatically when it changes on disk.
watch_config = true
```

Set `watch_config = false` to reload only on request. Saving that change still
reloads once, since watching was on when it was saved; turning it back on
takes a manual reload. If the file watcher cannot start, Huterm shows one
warning and manual reload keeps working.

To reload manually, choose **Reload Configuration** from the Huterm
application menu on macOS or the window menu, or press Cmd+Shift+, on macOS or
Ctrl+Shift+, on Linux with a US keyboard layout. GPUI represents these
chords as Cmd+< and Ctrl+<; on other layouts use the corresponding less-than
chord.

Reload rereads the configuration and the selected theme chain. It applies
fonts, padding, colors, and most other settings to every open window without
restarting shells or losing scrollback. Windows keep their size; visible grids
are recalculated, and hidden grids resize when selected. Where a setting
reloads differently, its section below says so.

An invalid or unreadable file leaves the running settings in place and shows
the configuration error, whether the reload was automatic or manual. Saving a
fixed file, even one identical to the last working version, reloads it and
clears the error. A file that disappears is left alone until it comes back,
because editors briefly remove it while saving; a manual reload of a missing
file shows an error. To return to the defaults, empty the file.

At startup, a file that is not valid TOML stops Huterm. A valid file with a
bad setting starts with the defaults, shows the error, and still honors its
`watch_config`, so saving the fix applies it.

## Shells

On macOS, Huterm launches `$SHELL -l` in the home directory, as a Finder launch
would. The packaged app also sets `LANG=en_US.UTF-8` when it inherits no locale
variable. On Linux, Huterm launches `$SHELL` in its own working directory.
Without `$SHELL`, it falls back to `/bin/zsh` on macOS and `/bin/sh` on Linux.

New tabs and panes start in the focused pane's directory when that directory
is local and still accessible. Remote, malformed, deleted, or inaccessible
paths use the normal launch directory instead:

```toml
[terminal]
new_tab_directory = "inherit" # inherit or default
```

A pane closes without confirmation when its shell exits, and closing a tab's
last pane closes the tab. Set `close_on_exit = false` in `[terminal]` to keep
exited panes for their history. An exited pane allows selection, copy,
scrolling, links, and resizing, but sends no input. Reloading this setting
affects only later exits.

## Panes

A tab splits into panes, each with its own shell; the split, focus, resize,
and zoom commands are in [Commands and keybindings](keybindings.md). Drag a
divider to resize the panes beside it. Shells reflow while you drag, and
other windows show the new sizes once you release.

Zooming shows one pane across its tab. By default each window zooms on its
own. With `zoom = "shared"` the tab owns the zoom, so every window shows the
same zoomed pane:

```toml
[panes]
zoom = "view" # view or shared
```

### Pane layouts

`arrange_panes` and the layout cycle start with five built-in layouts,
described in [pane layouts](keybindings.md#pane-layouts). Define your own
under `[panes.layouts]`, each a name and a layout template, and choose the
layouts the cycle visits with `layout_cycle`:

```toml
[panes.layouts]
dev = "columns(pane 55%, rows(pane 70%, columns(*)))"
pair = "rows(columns(pane, pane) 70%, columns(*))"
sides = "columns(rows(*) 25%, pane, rows(*) 25%)"

[panes]
layout_cycle = ["columns", "rows", "main_left", "main_top", "tiled", "dev"]
```

A template describes a layout for any number of panes. The built-ins are
templates too: `main_left` is `columns(pane 60%, rows(*))`.

- `columns(...)` places its children left to right, and `rows(...)` top to
  bottom.
- `pane` takes one pane.
- `*` takes the panes left once every `pane` has one. Several stars divide
  them as evenly as possible, and when they do not divide evenly the earlier
  stars take one more each. In `columns` or `rows`, a star's panes sit side
  by side in that split and share the star's space equally.
- `grid(*)` tiles its panes as the built-in `tiled` layout does.
- A child may end with a share from 1 % to 99 %, such as `pane 60%`.
  Children without one divide what the shares leave equally, and a star
  counts as one child however many panes it takes, so in `columns(pane, *)`
  the pane keeps half the width at any pane count. The shares must total
  less than 100 when some child has none, and exactly 100 when every child
  has one.

Panes fill the slots in template order, which is the order focus and swaps
follow. A slot that receives no pane drops out, and its siblings' shares
grow in proportion to fill its space. With six panes, `sides` puts three on
the left, one in the centre, and two on the right. With two panes its right
stack is empty, so the left pane and the centre pane take one third and two
thirds of the width. A template without a `*` holds no more panes than it
has `pane` slots: `arrange_panes` refuses a tab with more, with a notice,
and the cycle skips that layout.

A layout name is up to 32 lowercase letters, digits, and underscores,
starting with a letter, and cannot be a built-in's name. The file holds at
most 64 layouts; each template is at most 1 KiB, with splits nested at most
16 deep. `layout_cycle` defaults to the five built-ins in the table's order,
may name any built-in or user layout, and must not be empty or repeat a
name. A layout that breaks these rules, or a `layout_cycle` entry naming a
layout that does not exist, makes the whole configuration invalid. The
error names the key, and for a template that does not parse, the line and
column within the template where it goes wrong. Removing a layout the cycle
still names is such an error, as removing a quake profile that a global
shortcut uses is. A keybinding naming a missing layout fails only when it
runs, with a notice. A reload replaces the layouts and the cycle.

The palette's layout picker and the Settings keybinding editor list your
layouts after the built-ins.

The Pane layouts card in the Window and panes section of Settings lists the
built-ins with their templates, and edits your layouts and the cycle. New
layout asks for a name and writes the layout as `columns(*)`, which you then
type over; it does not add the layout to the cycle. A template field checks
the template as you type and shows where it goes wrong, saves on Enter or
when you leave the field, never saves a template that does not parse, and
Escape puts back the file's template. Rename writes one save: the layout
under its new name, the old name removed, every `[[keybinding]]` whose
`arrange_panes` `layout` argument named it, including one another editor
added since Settings read the file, and `layout_cycle` with the new name in
the old one's place. The renamed layout moves to the end of
`[panes.layouts]`, and comments on its old line are removed. When the save
cannot be written, such as for a binding in an inline `keybinding = [...]`
array, which Settings cannot edit in place, or for a layout another editor
changed meanwhile, Settings writes nothing and says why. Deleting a layout
that a keybinding or the cycle uses is refused, naming each; change those
first. The cycle lists every layout, the cycle's own first and in order:
turn a layout on or off with its switch, and move it with Move up and Move
down. The cycle keeps at least one layout.

## Tab bar

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
macos_native_menus = true # native macOS menus; ignored on Linux

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
edge. Vertical bars show an overlay scrollbar that widens when the pointer
rests on it. The new-tab button stays visible and follows the last tab in a
vertical bar.

With one tab the bar stays hidden unless `always_show = true`.
`auto_hide_in_fullscreen = true` hides the bar in fullscreen until the pointer
reaches its edge, whatever the tab count or `always_show`. The bar then slides
over the terminal without changing the grid and hides again when the pointer
leaves. Dragging a tab or resizing the sidebar keeps it open, and switching,
creating, or closing tabs reveals it for a second. A top bar beside a display
notch has nowhere to hide and stays in place.

In non-native fullscreen on a notched display, `notch = "left"` or `"right"`
moves a top bar beside the camera housing and gives its height back to the
terminal. `"off"` keeps the bar below the notch. Elsewhere the setting is
ignored.

Tabs show a small yellow dot before their title while a program holds the
foreground; exit, error, and bell indicators replace it. The window title is
`<active tab title> — Huterm`, so window switchers show the active tab. On
macOS the title bar shows the active tab's title in place of the application
name.

## Title bar tabs

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

## Tab labels and directories

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

## Menus and notices

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
confirmation's buttons, the About panel, and the Licenses view show key hints
only with `shortcut_hints = true`.

On macOS the window, tab, pane, and terminal menus, and a text field's
right-click menu, open as native macOS menus while
`macos_native_menus = true`, the default. They list the same
items with the same shortcuts, follow the theme's light or dark appearance,
and use the system's keys and dismissal. The Edit row becomes separate
**Copy** and **Paste** items, and a disabled item shows no reason. A native
menu shows the state from when it opened, and closes if its tab or pane
goes away. Set `macos_native_menus = false` for Huterm's own menus, which
Linux always uses.

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

Right-clicking a pane's title bar in a split tab focuses the pane and opens its
context menu:

- **Rename Pane…**, which sets the name the title bar shows in place of the
  pane's title. A blank name restores the title.
- **Copy Directory Path**, disabled with "Directory unknown" until the pane
  reports a working directory.
- **Split Right** and **Split Down**.
- **Zoom Pane**, or **Unzoom Pane** while the pane is zoomed.
- **Move Pane to New Tab** and **Move Pane to Another Pane…**
- **Close Pane**.

A tab without a custom name shows its focused pane's name. A named pane that
moves to a tab of its own gives the new tab its name, and a named tab of one
pane that joins another tab as a pane gives that pane its name. Undo puts a
moved name back.

**About Huterm** (`about`) opens a panel with the version, identifier,
platform, terminal engine, and build profile, plus the short source revision
when the build embedded one. On Linux the platform line includes the display
backend. **Copy Details** copies those lines as text. Close or Escape closes
the panel, and terminal input is blocked while it shows. Tab and the arrow keys
move focus between its buttons, and Enter presses the focused one; focus starts
on Close.

**Licenses** in the About panel lists the open source software this build
ships, with each component's version, license, and full license text. Type to
search by name, version, license, or kind; Up and Down select, Page Up and
Page Down scroll the text, and **Copy** copies it. Tab and Shift-Tab move focus
from the search to the **Website**, **Copy**, and **Done** buttons, and Enter
presses the focused one. Escape clears the search, then returns to the About
panel.

Errors and warnings appear as notices in the bottom-right corner of the
terminal. Configuration and keybinding diagnostics stay until a reload fixes
them. Command and terminal failures expire after six seconds unless hovered or
focused.

## Fullscreen

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

## Quake windows

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
# disabled = true           # keep the entry without registering it
```

The `default` profile exists without configuration. Add named tables, such as
`[quake.profiles.logs]`, for independent windows. The Quake windows section of
Settings picks a profile from a dropdown, adds, renames, and deletes named
profiles, and records global shortcuts, each with a switch that sets
`disabled`. It edits profiles whose names are bare
TOML keys (letters, digits, `-`, and `_`); others, such as
`[quake.profiles."my logs"]`, are listed but stay in the file, where you edit,
rename, or delete them. `show_quake`, `hide_quake`, and `toggle_quake` accept
an optional `profile` argument and also work in ordinary `[[keybinding]]`
entries. A hidden window keeps its tabs, shells,
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

Renaming a profile in Settings writes one save: the profile's table under its
new name, the old table removed, and the `profile` argument of every
`[[global_keybinding]]`, and of every `[[keybinding]]` running a quake command,
that named the old name changed to the new one, including a shortcut another
editor added since Settings read the file. The table is written afresh before
the file's first `[[...]]` entry, or at its end when it has none, rather than
where the old table stood, so comments on its lines and directly above its
header are removed; a comment on a rewritten shortcut's line stays. An open
quake window of the profile stays open as that profile's window under the new
name, with its tabs and shells. A deleted profile's window keeps its name until
it has become an ordinary window, so renaming a profile to that name is refused
until then. When the save cannot be written, such as for a shortcut in an
inline `global_keybinding = [...]` array, which Settings cannot edit in place,
or for a profile another editor changed meanwhile, Settings writes nothing and
says why, and changes made to the new name's settings while the rename waited
are not written either. Deleting a profile that a
global shortcut uses is refused; deleting one that a `[[keybinding]]` runs
warns first, since the file still accepts that binding and it fails only when
pressed.

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
An unsupported key or an invalid binding is a configuration error, and a
rejected reload keeps the previous configuration and shortcuts. A shortcut
another application already holds is only a warning: the rest of the
configuration applies, the other shortcuts register, and the next reload,
including the one a save starts, tries it again. Global shortcuts work on macOS
and native X11 sessions, but not on Wayland or XWayland, where configuring one
warns the same way.

## Fonts and themes

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

See [theme sources and licenses](../crates/huterm-gpui/themes/README.md) for
attribution and palette import details.

## Scrollback

Each terminal keeps the output that scrolls off its screen, up to a limit in
screen rows and, optionally, a limit on size. Whichever is reached first
applies.

```toml
[terminal]
scrollback_rows = 10000       # 0 through 100000; 0 keeps no history
scrollback_bytes = "64MiB"    # optional; up to "16GiB"; 0 keeps no history
scrollback_compression = true # compress history while the terminal is idle
```

A long line that wraps across three rows counts as three rows. Huterm frees
history a page at a time, oldest first, so both limits are ceilings rather
than exact counts:

- Once output passes the row limit, a terminal keeps at most `scrollback_rows`
  rows, and less than one page fewer. A page holds about 570 rows at 80
  columns and about 230 at 200 columns, and more on narrower terminals.
- A nonzero row limit smaller than one page keeps up to one page.
- `0` in either setting keeps no history, whatever the other says.

`scrollback_bytes` takes a byte count or a string of digits and a unit, with at
most one space between them: `B`, `KB`, `MB`, and `GB` count in powers of 1000,
and `KiB`, `MiB`, and `GiB` in powers of 1024. Units ignore case, so `b` also
means bytes. Fractions such as `"1.5GiB"` are not accepted. The size covers the
memory Ghostty allocates for the terminal's grid: the visible screen as well as
its history, so a taller window leaves less of it for history. Ghostty always
allows the visible screen rounded up to whole pages, plus one more page, so a
very small value still keeps up to about two pages of history, depending on how
the screen fills its pages; on a tall, wide window that can be far less than one
page. It is a page-granular limit, not a memory budget: Ghostty checks it when
it allocates a page and when a limit is applied, so pages that grow for
combining characters or styles can push history past it until then, and it does
not count memory outside the grid.

Without `scrollback_bytes`, Huterm still gives Ghostty a size allowance, so
that unusual output cannot grow a terminal without bound: 4 KiB for each
configured row and each visible row, plus 4 MiB, about 45 MB at the default.
It follows the window's height as it resizes. Plain output up to about 400
columns wide stays within it, so the row limit applies. Wider rows, or rows
with many distinct styles or combining characters, can reach it first and keep
fewer rows than the limit. Setting `scrollback_bytes` replaces this allowance,
and the configured size stays fixed as the window resizes.

The 100,000-row maximum holds even under a large `scrollback_bytes`. It keeps
resizing quick: a change in width rewraps every row of history, which pauses
that terminal for about 7 ms at the maximum on an Apple silicon Mac. Plain
history at 120 columns takes about 1 KB per row, so a full 100,000 rows uses
about 100 MB per terminal before compression, and wider rows use more.

`scrollback_compression`, on by default, compresses each terminal's history
once nothing has changed in it for a quarter of a second and you have stopped
typing. Text-heavy history shrinks to about a tenth of its memory: a full
100,000 rows of build output or file listings drops to 5 to 13 MB. Output with
a different color on every word or cell compresses about half as well.
Compressed history reads exactly as before. Scrolling into it, selecting it,
or looking up a link in it restores only the pages involved, and the next
idle period compresses them again. The limits above count uncompressed
history, so compression changes how much memory history uses, not how much
history a terminal keeps.

The first change in width after compression restores all history before
rewrapping it, which takes two to three times as long as an uncompressed
rewrap: about 30 ms instead of 10 ms for a full 100,000 rows of build output
on an Apple silicon Mac, and a few milliseconds at the default limit. Further
resizes during the same drag cost the usual amount. On macOS, Activity
Monitor's **Memory** column shows the saving at once. Tools that report
resident size, such as `ps` and `top`, show it only when the system needs the
memory back. Reload applies the setting to every open terminal. Turning it
off stops further compression; history already compressed stays compressed
until something reads it.

Reload applies new limits to every open terminal, including tabs in the
background and tabs whose shell has exited. Lowering either limit discards the
oldest rows at once. Raising one lets history grow again but does not restore
rows already discarded. A raise usually discards nothing and leaves views and
selections as they were, but when grown pages have pushed history past the size
limit, applying any new limit discards the oldest rows. The limits apply to the
normal screen's history even while a full-screen program such as vim shows the
alternate screen. Narrowing a window or enlarging its font rewraps long lines
onto more rows, and widening it makes each row larger, so either can also
discard the oldest rows. When history shrinks, a view scrolled into history
stays on the same rows if they remain and otherwise moves to the oldest row
left, and any selection is cleared. **Clear Scrollback** and **Reset Terminal**
keep the limits.

These settings cover only the history a running terminal holds in memory.
Huterm does not save history across restarts; saved history will have its own
limit.

## Mouse, selection, and scrolling

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

## Links and file drops

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

## Clipboard writes from applications

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

## Option and Alt as Meta

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

## Terminal compatibility

These sections describe how Huterm presents itself to programs, and the queries
it answers.

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

## Engine and refresh

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

## Updates

Release builds for macOS check for updates with Sparkle; see
[Install](../README.md#macos). To manage scheduled checks from the
configuration file, set either key in the `[updates]` table:

```toml
[updates]
automatic_checks = true
check_interval_hours = 24
```

An explicit `true` or `false` turns scheduled checks on or off without
Sparkle's prompt. Setting only the interval does not give consent. Leave a key
out to keep the value Sparkle has stored. Development builds, packages built
with `mise run package:macos`, and Linux builds do not include Sparkle and
ignore this table.
