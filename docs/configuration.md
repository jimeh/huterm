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
settings in every section. Settings marked Advanced appear in every section
once you turn on Show advanced settings at the foot of any section, or the
switch in General; either writes `show_advanced`, and hiding them removes it.
The sidebar's sections scroll when the window is too short to list them all,
and moving between them with the keyboard keeps the selected one in view.

```toml
[settings]
show_advanced = false # show the settings marked Advanced
```

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

Each pane of a split tab has a title bar with its title, zoom and close
buttons, a drag handle, and a context menu. `title_bar_position` puts the bars,
and the focus accent described below, along the top or bottom of their panes.

`title_bar = "hover"` gives the terminals the bars' space and draws a pane's bar
over its terminal only when you want it: rest the pointer where the bar would be
for a moment and it fades in, ready to click, drag, or right-click, and it fades
out again once the pointer, a drag, or its menu leaves it. Until it shows, the
pointer reaches the terminal there as usual. Moving focus between panes with
the keyboard shows the newly focused pane's bar for a second. In fullscreen with
an auto-hiding tab bar, the tab bar's trigger edge wins over the bars below it.
Three settings, in milliseconds, time hover bars: `title_bar_hover_delay_ms` is
how long the pointer rests before a bar appears, from 0 to 2000;
`title_bar_fade_ms` is how long a bar takes to fade in or out, from 0 to 1000;
and `title_bar_hide_delay_ms` is how long a bar stays after the pointer leaves,
from 0 to 5000. They apply only to pane title bars; the fullscreen tab bar
keeps its own timing.

`title_bar = "hidden"` removes the bars and gives their space to the terminals;
closing, zooming, moving, and renaming panes stay available as commands.

Two indicators show which pane has focus while more than one pane is on
screen; each is off at 0. `focus_accent` draws a thin accent line along the
focused pane's edge where its title bar would be, at that opacity from 0 to 1,
and is off by default. It draws only under hover or hidden bars, and under
hover bars only until the focused pane's bar appears. `inactive_dim` dims every
other pane's terminal by laying the theme's background color over it, from 0
to 0.8, and defaults to 0.2; their title bars stay as they are, and clicks,
selections, scrolling, links, and drops reach a dimmed pane as usual. All
seven settings apply on reload:

```toml
[panes]
title_bar = "visible" # visible, hover, or hidden
title_bar_position = "top" # top or bottom
title_bar_hover_delay_ms = 75 # 0 to 2000
title_bar_fade_ms = 75 # 0 to 1000
title_bar_hide_delay_ms = 400 # 0 to 5000
focus_accent = 0.0 # 0 (off) to 1
inactive_dim = 0.2 # 0 (off) to 0.8
```

### Modifier pane drags

`drag_modifier` lets you drag a pane by pressing anywhere in it while you hold
a modifier combination, which helps when title bars are hidden or out of
reach. It defaults to `cmd-ctrl` on macOS and `ctrl-alt` on Linux, where many
desktops move windows with a Super combination; `"off"` turns it off. Name two
or more of `cmd` (or `super`, its Linux spelling), `ctrl`, `alt`, and `shift`,
joined with hyphens:

```toml
[panes]
drag_modifier = "cmd-ctrl" # Linux default: "ctrl-alt"; "off" turns it off
```

While the combination is held over a pane of a split tab in the active window,
the pointer shows an open hand, and after a moment every pane shows its title
in a badge at its centre. A key press while the combination is held, such as a
pane keybinding, hides the badges until the modifiers change. A press then
starts the same drag a title bar does, with a closed hand, the same drop
targets, and Escape to cancel; letting the modifiers go mid-drag changes
nothing. A press without movement only focuses the pane.

The combination's click no longer reaches the terminal of a split tab: no
selection, link, or mouse report. On macOS `cmd-ctrl` replaces the
Command-Control-click context menu there; a right-click or a plain
Control-click still opens it. A tab of one pane cannot be dragged this way,
so the combination's press reaches its terminal as before, and so does the
press that activates an inactive window. Title bars, dividers, notices, and
menus over a pane keep their own presses. A desktop's own window-move modifier,
such as Super on GNOME, takes the press before Huterm sees it.

Huterm refuses a combination that would take another gesture. A single
modifier is refused: Command opens links on macOS and Control on Linux,
Control-click is a right-click on macOS, Shift bypasses mouse reporting, and
Option and Alt clicks stay free for pointer conventions. macOS also refuses
`ctrl-alt`, which VoiceOver uses, and `ctrl-shift`, whose click opens the menu
inside programs that take the mouse. The combination may not be
`terminal.link_modifiers`, or that chord plus `shift`. A file that breaks
these rules does not load, and a reload keeps the running settings. When
`drag_modifier` is unset and a custom link chord is the default combination,
such as `link_modifiers = "ctrl-alt"` on Linux, modifier drags stay off
instead. Settings offers only the combinations that pass, writes `cmd` on
macOS and `super` on Linux, and writes `"off"` for Off.

### Pane layouts

`arrange_panes` and the layout cycle start with six built-in layouts,
described in [pane layouts](keybindings.md#pane-layouts). Define your own
under `[panes.layouts]`, each a name and a layout template, and choose the
layouts the cycle visits with `layout_cycle`:

```toml
[panes.layouts]
dev = "columns(pane 55%, rows(pane 70%, columns(*)))"
pair = "rows(columns(pane, pane) 70%, columns(*))"
sides = "columns(rows(*) 25%, pane, rows(*) 25%)"

[panes]
layout_cycle = [
  "columns", "rows", "main_left", "main_top", "main_centered", "tiled", "dev",
]
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
grow in proportion to fill its space. `sides` arranges panes as the built-in
`main_centered` does: with six panes it puts three on the left, one in the
centre, and two on the right. With two panes its right
stack is empty, so the left pane and the centre pane take one third and two
thirds of the width. A template without a `*` holds no more panes than it
has `pane` slots: `arrange_panes` refuses a tab with more, with a notice,
and the cycle skips that layout.

A layout name is up to 32 lowercase letters, digits, and underscores,
starting with a letter, and cannot be a built-in's name. The file holds at
most 64 layouts; each template is at most 1 KiB, with splits nested at most
16 deep. `layout_cycle` defaults to the six built-ins in the table's order,
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

The Pane layouts card in the Panes section of Settings lists the
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

## Size indicator

While a window or pane resizes, and when a pane splits, a panel in the middle
of each resized terminal shows its size in columns and rows. `size_indicator`
chooses it: `"full"` shows the panel, or a compact badge with the numbers alone
in a pane too small for the panel; `"compact"` always shows the badge; and
`"off"` shows neither. A tiny pane shows nothing. The indicator stays for
`size_indicator_linger_ms` after the size stops changing, from 0 to 10000
milliseconds, then fades. While you still drag a pane divider or the sidebar's
edge, or on macOS a window's edge, it stays however long you rest, and the
linger starts when you release. On Linux, Huterm cannot tell when a window
edge drag ends, so there the linger counts from the last resize. Both settings
apply on reload:

```toml
[window]
size_indicator = "full" # full, compact, or off
size_indicator_linger_ms = 100 # 0 to 10000
```

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
size_indicator = "full" # full, compact, or off; see Size indicator
size_indicator_linger_ms = 100

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
`disabled`. The Keybindings section lists the same global shortcuts under Show
Quake, Hide Quake, and Toggle Quake, tagged Global, and edits the same
entries; its Add global shortcut writes one for that command on the `default`
profile. It edits profiles whose names are bare
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

Each show of a hidden quake window picks its display again, so after you
connect or disconnect a display, or change which one is primary, the next show
follows `display`: the display of the application you summoned it from, the
one under the pointer, the current primary display, or the display with that
ID. A visible window stays on its display while that display is connected.
When it is disconnected, the window moves to the display `display` names, or
to the primary display for `active` and `pointer`. An ID that names no
connected display uses the primary display, or the first display when none is
primary, and logs a warning.

Moving the window to another display in regular presentation keeps it there
across hides until a display is connected or disconnected, the primary display
changes, the profile's `display` changes, or the window closes. A change in
resolution keeps it. Explicit IDs use CoreGraphics display UUIDs on macOS and
RandR monitor names on X11.

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
ui_family = ""   # empty uses the system interface font
ui_size = 13.0   # 10 through 16
```

`family` and `size` set the terminal grid. `ui_family` and `ui_size` set
Huterm's own interface: tabs, pane title bars, Settings, the command palette,
the menus and dialogs Huterm draws, notices, and other overlays. Native macOS
menus keep the system font, and text set in a monospace family, such as
process names, keeps that family. A family that is not installed falls back
to the system interface font, and Settings says so under the field, as it does
for a terminal `family` that falls back to the platform's monospace font.
Settings lists the installed fonts again when it opens, comes forward, or
follows a reload, without holding up the window. On macOS a family installed
while Huterm runs then shows in the list, and when the file names it, Huterm
reloads so it applies and the note goes. On Linux the font list is read once
at startup, so a newly installed font needs a restart.

At the default `ui_size` of 13, the interface draws at its design sizes.
Other sizes scale its text, and the controls, rows, and menus that hold text,
in proportion, so 16 draws everything about 23% larger. Terminal windows'
tab bars and pane title bars keep their heights, which is why the range stops
at 16. A value outside 10 through 16 is an error that names the key. Both
keys apply to open windows when the file reloads.

Select a built-in theme and optionally override individual colors:

```toml
[theme]
name = "catppuccin-mocha"
background = "#181825"
ansi_red = "#f38ba8"
ansi_bright_red = "#eba0ac"
```

Without a `name`, the theme is `tokyo-night`, and any color overrides apply
on top of it.

To follow the system's light or dark mode, set a theme for each:

```toml
[theme]
light = "github-light"
dark = "tokyo-night"
```

Setting `light` or `dark` turns this on; a mode without a theme uses `name`,
or `tokyo-night` without one. A `name` must still be a theme that exists
when both modes replace it. Either mode can use any theme, light or dark.
Huterm switches themes in every window as soon as macOS or the Linux
desktop changes mode. On Linux the mode comes from the desktop portal's
color scheme, so a desktop without one counts as light, and a dark desktop
can show the light theme for a moment while Huterm starts. Color overrides
in `[theme]` apply in both modes. In Settings, the **Theme mode** row in
Appearance switches between them: **Match system** writes both keys, the
Light mode and Dark mode cards under it choose which mode the theme cards
set, and **One theme** keeps the theme in use now as `name`.

The built-in themes are embedded in the application:

- Light: `ayu-light`, `catppuccin-latte`, `everforest-light`,
  `github-light`, `gruvbox-light`, `kanagawa-lotus`, `one-light`,
  `rose-pine-dawn`, `solarized-light`, and `tokyo-night-day`.
- Dark: `tokyo-night` (the default), `tokyo-night-storm`, `tokyo-night-moon`,
  `ayu-dark`, `ayu-mirage`, `catppuccin-frappe`, `catppuccin-macchiato`,
  `catppuccin-mocha`, `dracula`, `everforest-dark`, `github-dark`,
  `gruvbox-dark`, `huterm-dark`, `kanagawa-dragon`, `kanagawa-wave`, `nord`,
  `one-dark-pro`, `rose-pine`, `rose-pine-moon`, `solarized-dark`,
  `tango-with-monokai`, and `tomorrow-night`.

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
its resulting colors rather than from Huterm Dark's hand-picked set. Overrides
in the config's `[theme]` table apply last. Named definitions and theme files
use `extends`; `name`, `light`, and `dark` belong to the config's `[theme]`
alone.

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

On macOS the window frame, its title bar and traffic lights, and native menus
follow the theme rather than the system's light or dark mode: a theme with a
light background gets the light frame even while macOS is dark, and the other
way round. Every window follows, including Settings and Quake windows, and a
theme change, reload, or switch between `light` and `dark` updates them.

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

Over the terminal the pointer is a text cursor where a click selects, and an
arrow over the scrollbar and while an application has the mouse. Hold Shift
when starting a selection or scrolling to use local selection and scrollback
while an application has the mouse; the text cursor shows while Shift is
held. The scrollbar always stays local. Mouse input also stays local while you
view scrollback, including while a scroll away from live output waits for its
snapshot; return to the live view to interact with the application again. A
drag keeps its original owner until release, even if Shift changes or the
pointer leaves the grid.

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

With [`panes.drag_modifier`](#modifier-pane-drags) set, a press made while
holding that combination in a split tab's pane drags the pane instead of
reaching the terminal.

When a program such as vim or tmux has the mouse, a plain right-click goes to
the program and Shift+right-click opens the menu. `terminal.right_click`
chooses another action: `"menu"` (the default), `"paste"`, `"copy_or_paste"`
(copy and clear a selection, otherwise paste), or `"ignore"`. Menus opened at
the pointer shift up or down to fit the window and scroll only when taller
than it.

Bells never select a tab, activate a window, or move focus. On macOS a bell
plays a sound by default; the visual bell is off. Turning on `visual` makes a
bell briefly flash the active terminal and mark an inactive tab until you view
it. Reloading `visual = false` clears existing bell marks:

```toml
[terminal.bell]
visual = true
```

On macOS, `audio` (on by default) plays a sound when a bell rings in any
terminal, focused or not, in a visible tab or a hidden one. Huterm plays at
most one sound every 200 ms across all terminals, so a burst of bells plays
once. `sound` is `"system"`, the alert sound chosen in macOS's Sound settings,
or one of the sounds in `/System/Library/Sounds`: `"basso"`, `"blow"`,
`"bottle"`, `"frog"`, `"funk"`, `"glass"`, `"hero"`, `"morse"`, `"ping"`,
`"pop"`, `"purr"`, `"sosumi"`, `"submarine"`, or `"tink"`. `volume`, from 0
to 1, sets how loud a named sound plays; the system alert sound always plays
at the macOS alert volume. A named sound that fails to load plays the alert
sound instead. Set `audio = false` to silence bells. Linux has no audible
bell: `audio` defaults to `false` there, `audio = true` loads with a warning,
and bells stay silent.

```toml
[terminal.bell]
audio = true
sound = "glass"
volume = 0.5
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
their Alt-modified escape sequences with every setting.

`"left"` and `"right"` send Meta from that Option key only, so the other one
still types macOS characters and dead keys. Huterm tells the keys apart from
the side bits macOS attaches to each key press. Some key remapping tools and
Screen Sharing leave those bits out, and then neither key sends Meta.

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

New shells use `TERM=xterm-256color` by default. Nearly every system,
including remote hosts reached over SSH, has that terminfo entry. Huterm also
sets `COLORTERM=truecolor`, which tools such as tmux and Emacs use to enable
24-bit color without a dedicated terminfo entry. SSH does not forward
`COLORTERM` unless the client's `SendEnv` and the server's `AcceptEnv` allow
it, so remote programs may fall back to 256 colors.

Huterm also carries a private `xterm-huterm` terminfo entry. Development tasks
compile it into `target/terminfo`, and the macOS and Linux packages carry the
compiled entry. Opt into it with `auto`, which uses `xterm-huterm` when the
entry is available locally and `xterm-256color` otherwise, or with
`xterm-huterm`, which always uses it:

```toml
[terminal]
term = "xterm-256color" # Or "auto" / "xterm-huterm" for new shells.
```

Huterm 0.10 through 0.25 wrote `term = "auto"` into the configuration files
they created. Remove that line, or set it to `xterm-256color`, to use the new
default.

`xterm-huterm` inherits the standard 256-color capabilities and advertises
`Tc`, for terminfo consumers that read 24-bit color support only from
terminfo. If you choose `xterm-huterm`, making the entry available is your
responsibility. A missing entry usually appears as an "unknown terminal type"
error in curses applications.

With `auto` or `xterm-huterm`, SSH passes `xterm-huterm` to the server, and
local discovery cannot tell whether the server has the entry. zsh does not warn
about a missing entry. Its line editor loses cursor movement and line clearing,
so redrawn input piles up on the line: zsh-autosuggestions text stays behind
and later keystrokes land after it, often with doubled characters. Check on the
remote host with `infocmp -x "$TERM"`.

When a server lacks the entry, use a compatible identity for one SSH
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
