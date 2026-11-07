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
settings in every section by name, description, or group heading. Settings
marked Advanced appear in every section once you turn on Show advanced
settings at the foot of any section, or the switch in General; either writes
`show_advanced`, and hiding them removes it.
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
Sliders that read as a percent, such as the opacities and a Quake profile's
width and height, move in 1% steps: the arrow keys step them by 1%, Page Up
and Page Down move 10%, and Home and End go to the ends of the range, 1% and
100% for a Quake width or height. A value the file sets off that grid shows
as it is, and the first step moves it onto the grid. Sliders with a unit,
such as blur, page by a tenth of their range.

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
`watch_config` and `restore_sessions`, so saving the fix applies it.

## Restoring sessions

Quitting Huterm and starting it again reopens every session that was open,
with its windows, workspaces, tabs, split panes and their sizes, custom
names, focused panes, window positions, and per-window transparency. Each
window reopens on the workspace it showed and remembers the tab and pane it
last showed in every other workspace. Each pane starts a
fresh shell in the folder it last reported; nothing that ran before is
started again. A pane whose last folder no longer exists starts in an
earlier folder it used, or failing that in your home folder on macOS or
Huterm's working directory on Linux, and its window says so. A session no
window showed at Quit comes back the same way, running but in no window;
switch to it from the session menu. Closing the
last window is not a Quit: it ends that session, so the next launch opens
one fresh window.

While Huterm runs it also saves a checkpoint one second after windows,
tabs, panes, or a shell's folder change, so a crash or a killed process
restores the last checkpoint.

```toml
[general]
# Reopen the windows, tabs, and panes open at Quit, each pane with a fresh
# shell in its last folder. Turning this off deletes the saved windows.
restore_sessions = true
```

Set `restore_sessions = false` to start fresh every time. Turning it off,
in the file or in Settings > General, deletes the saved windows at once;
turning it on saves the current windows right away. Both apply without a
restart.

Saved state lives in `$XDG_STATE_HOME/huterm/` (default
`~/.local/state/huterm/`) on both platforms, or in `$HUTERM_STATE_DIR` when
it is set to an absolute path. Only one Huterm process saves sessions: a
second one started while the first runs neither restores nor saves its
windows, and says so. When the saved state cannot be read, or a restore did
not finish, Huterm opens a fresh window, keeps the bad file as
`runtime.json.corrupt`, and shows why. State saved by a newer Huterm is left
alone and not restored.

## Session numbering

A session you have not named shows an automatic name such as "Session 2".
By default a new session takes the lowest number no open session uses, so
after you close "Session 2" the next new session is "Session 2" again. A
session you renamed keeps its number for Clear Name, so that number stays
taken.

```toml
[general]
# "lowest_unused" or "increasing".
session_numbering = "lowest_unused"
```

Set `session_numbering = "increasing"` to keep counting up instead: each
new session takes one more than the highest number used since launch, and
a restart continues after the highest number it restored. Workspaces
number the same way. The setting applies to sessions created after it
changes, without a restart; existing names never change.

## Shells

On macOS, Huterm launches `$SHELL -l` in the home directory, as a Finder launch
would. The packaged app also sets `LANG=en_US.UTF-8` when it inherits no locale
variable. On Linux, Huterm launches `$SHELL` in its own working directory.
Without `$SHELL`, it falls back to `/bin/zsh` on macOS and `/bin/sh` on Linux.

New tabs and panes start in the focused pane's directory when that directory
is local and still accessible. When it is remote, as while `ssh` runs, or
malformed, deleted, or inaccessible, they start in the last local directory
the pane's own shell reported, if that is still accessible, and otherwise in
the normal launch directory. New windows always use the normal launch
directory:

```toml
[terminal]
new_tab_directory = "inherit" # inherit or default
```

A pane closes without confirmation when its shell exits, and closing a tab's
last pane closes the tab. Set `close_on_exit = false` in `[terminal]` to keep
exited panes for their history. An exited pane allows selection, copy,
scrolling, links, and resizing, but sends no input. Reloading this setting
affects only later exits.

## Shell integration

Huterm loads prompt and working-directory integration in the bash, zsh, fish,
and nushell process it starts. It reports OSC 7 directories and OSC 133 prompt,
input, execution, and completion markers, including the command's exit status.
These directory reports also feed **Copy Directory Path** and the existing
`terminal.new_tab_directory = "inherit"` behavior.

```toml
[shell_integration]
mode = "detect" # detect or off; default: detect
features = []   # opt in with ["path"]
```

`mode` is available in Settings under Terminal. Edit `features` in the file.
Unknown features are errors, and duplicate names collapse. Reload changes only
terminals started afterwards. The `path` feature appends `HUTERM_BIN_DIR` to
`PATH` after startup files have run, only if that directory is missing. It does
not add a command-line interface.

| Shell | Loading and startup order |
| --- | --- |
| zsh | A static `ZDOTDIR` stub in Huterm resources restores the original value, reads the user's `.zshenv`, and installs hooks. Native `.zprofile`, `.zshrc`, and `.zlogin` ordering follows. |
| bash | Every bash, Apple's `/bin/bash` 3.2 included, starts as `bash --rcfile <bootstrap> -i` with the other options it was given. For a login shell (`-l`, `--login`, or `l` in combined flags such as `-il`), the bootstrap emulates the login sequence: `/etc/profile`, then the first readable `.bash_profile`, `.bash_login`, or `.profile`. Other shells read `.bashrc` or the requested `--rcfile` as bash does. `--noprofile` is honored. `--norc` starts bash without integration, because an rcfile start cannot skip a distribution's system bashrc. Bash keeps its own `ENV`, `HISTFILE`, and option defaults. |
| fish | A vendor `conf.d` bootstrap runs after user `conf.d` snippets and before `config.fish`. Fish 4.9.3 provides the full marker stream natively. |
| nushell | A vendor autoload module and `--execute 'use huterm *'` preserve native startup order. Nushell 0.116.1 supplies OSC 133; Huterm replaces enabled native OSC 7 with literal directory reports. |
| elvish | Huterm adds a module directory. Add the guarded `use` below to `rc.elv`; Elvish has no automatic loading hook. |

For elvish, add this to `rc.elv`:

```elvish
if (has-env HUTERM_RESOURCES_DIR) {
    try { use huterm-integration } catch { }
}
```

Only the initial shell is admitted automatically. A nested shell, `exec`, `su`,
or `sudo` starts without Huterm's injection. Subshells such as `( ... )` or
`$( ... )` inherit functions but run no prompt hooks. For integration in nested
shells, source the body explicitly from a startup file. Repeated sourcing in
one process does not install duplicate hooks.

Bash preserves the user's history policy, including `HISTCONTROL=ignorespace`,
and uses its normal unexported `HISTFILE=$HOME/.bash_history` default when no
history file was inherited. Huterm's entries in `PROMPT_COMMAND` sit on lines
(or array elements) of their own: the prompt decoration is last, and on bash
3.2 or with `promptvars` off a status capture is first. Huterm rebuilds this
layout at every prompt, so an installer that replaces or appends to
`PROMPT_COMMAND` at the first prompt keeps its entries. With bash-preexec
loaded, the capture follows its first entry and the decoration precedes its
last one, or its pending 0.7 installer. Each entry runs only in the shell that
loaded Huterm: a nested shell that inherits an exported `PROMPT_COMMAND` skips
it without an error and with its `$?` and `$_` unchanged.

Bash 4.4 and later emit the execution marker from `PS0`. With `promptvars` on,
the completion marker is part of the prompt string and expands `$?`, which bash
restores to the command's status before expanding the prompt. User
`PROMPT_COMMAND` entries and the prompt therefore see the same `$?`, `$_`, and
`PIPESTATUS` as in a plain shell. Only Huterm's prompt hook arms `PS0`'s
execution marker, and `PS0`'s expansion disarms it, so a command gets that
marker only after a prompt Huterm decorated, and empty input gets no execution
or completion marker. With `promptvars` on, Huterm's additions to `PS1` and
`PS0` expand Huterm's own arrays, which bash never exports, so a nested shell
that inherits an exported `PS1` or `PS0` shows none of Huterm's markers.
Huterm puts back the prompt it decorated only while `PS1` or `PS0` still holds
the exact value Huterm set; a prompt that the user or a theme replaced is
decorated as it is. With `promptvars` off, the first entry captures the status
and restores `$?` and `$_` for the entries after it, a rise in the prompt
command number (`\#`) marks a command, and the prompts carry static markers.
Bash 3.2 marks execution from a `DEBUG` trap, writing the marker to the
terminal so that a redirected first command such as `{ ...; } > file` does not
capture it. When a `DEBUG` trap exists after the startup files, on any version,
Huterm's trap runs it verbatim for every simple command except Huterm's own,
with the previous status, `$_`, and `extdebug` result. Preexec-style traps,
such as prompt timers, therefore see only user commands. Huterm does not use
bash-preexec's callbacks or change its history policy.

Zsh keeps a status hook first in `precmd_functions`, which reports completion
and the directory, and a decoration hook last, so it wraps the prompt that the
other hooks build. Huterm restores both positions before each command, and a
startup file that assigns the hook arrays gets Huterm's hooks back at the first
prompt. Commands see the prompt without Huterm's markers. Huterm puts the
undecorated prompt back only while `PROMPT` still equals the value Huterm set,
so it never removes a framework's markers or a prompt the user changed. When
other hooks follow the decoration hook, as on the first prompt after startup
files add theme hooks, decoration runs as a `zsh/sched` command after all
`precmd` hooks and before the prompt is expanded. Huterm adds no ZLE widgets
and never resets the prompt itself, so `prompt_subst` commands expand once.
Some themes rebuild `PROMPT` while the line is being edited and redraw it with
`zle reset-prompt`: Spaceship's asynchronous sections, Powerlevel10k's
transient prompt, and vi mode indicators do this. For those redraws, Huterm
defines a `zle` function that passes every call to the builtin. During a
`reset-prompt` call, it adds the markers to a prompt that has none, then gives
the theme its own `PROMPT` value back. With `prompt_percent` unset, zsh cannot
mark text as zero-width, so Huterm prints the prompt marker directly and omits
the input marker. Both shells detect framework markers in the final prompt
string and keep OSC 7 and `path` when they suppress OSC 133.

Fish, nushell, and elvish remove only Huterm's entry from `XDG_DATA_DIRS` if a
startup file changed its value. If it still matches the injected value, they
restore the inherited value exactly, including unset or empty. An unset or
empty inherited value searches the XDG defaults `/usr/local/share:/usr/share`
during startup. Fish user `conf.d` snippets run before Huterm's vendor
bootstrap, so their additions are preserved during cleanup.

Huterm exports `HUTERM_RESOURCES_DIR` and `HUTERM_BIN_DIR` even with `mode =
"off"` and for unsupported shells. If resources cannot be found, the resources
variable is absent. Use a guard when sourcing manually:

```bash
# ~/.bashrc
if [ -n "${HUTERM_RESOURCES_DIR:-}" ]; then
    source "$HUTERM_RESOURCES_DIR/shell-integration/bash/huterm.bash"
fi
```

```zsh
# ~/.zshrc
if [[ -n ${HUTERM_RESOURCES_DIR-} ]]; then
    source "$HUTERM_RESOURCES_DIR/shell-integration/zsh/huterm.zsh"
fi
```

```fish
# config.fish
if set -q HUTERM_RESOURCES_DIR
    source "$HUTERM_RESOURCES_DIR/shell-integration/fish/huterm.fish"
end
```

Nushell's `source` needs a parse-time constant. Put this in `config.nu`:

```nu
const huterm_body = '/path/to/resources/shell-integration/nushell/huterm.nu'
source $huterm_body
```

Replace `/path/to/resources` with the value of `HUTERM_RESOURCES_DIR`. Use that
literal only in a Huterm-specific config, or when the same path is installed
wherever that config runs. A runtime `if` cannot protect an unavailable source
file, and `$env` is not a parse-time constant. Nushell's native OSC 133 remains
controlled by `$env.config.shell_integration.osc133`. If
`$env.config.shell_integration.osc7` is true when Huterm's module loads, Huterm
turns it off and reports `kitty-shell-cwd://<host><path>` at each prompt and on
PWD changes. This preserves spaces, `#`, `?`, and `%` that nushell 0.116.1 puts
unencoded in `file://` reports. Reports are skipped when the host or path
contains a control character. If `osc7` is false at load, Huterm emits no OSC 7.

Limitations:

- A bash login shell is emulated, on every platform and version: `shopt
  login_shell` is off, `logout` refuses, and `.bash_logout` does not run.
  Non-login starts read `.bashrc` as bash itself does.
- Apple bash 3.2's `DEBUG` trap reduces `PIPESTATUS` to the last command's
  status. For example, after `false | true`, it reports `0` instead of `1 0`.
  Every DEBUG-trap integration on that version, including bash-preexec, has
  this limitation. Bash 4.4 and later keep the full pipeline status for
  commands and prompts.
- A user `DEBUG` trap that exists after the startup files runs inside Huterm's
  trap, on every bash version, and sees `PIPESTATUS` holding only the previous
  command's status. A trap that uses `extdebug` to skip commands based on
  `PIPESTATUS` can therefore decide differently than in a plain shell.
- With `promptvars` off and a scalar `PROMPT_COMMAND`, Huterm's status capture
  runs first, so user prompt hooks see `PIPESTATUS` holding only the previous
  command's status. Turning `promptvars` off during a session, with `shopt -u
  promptvars`, gives that one command a completion marker without a status.
- At the first prompt, bash's `$_` and `PIPESTATUS` come from Huterm's
  bootstrap sourcing the last startup file: `$_` is that file's path. `$?`
  matches a plain start. In zsh, a prompt decorated by the scheduled command
  shows only `$?` in `pipestatus`. That happens at the first prompt when
  startup files add `precmd` hooks or assign the hook arrays, and after a
  command that adds `precmd` hooks.
- A user `DEBUG` trap sees three bootstrap statements after the startup
  files: the one that saves the trap, the one that sets it aside, and the
  final `: "$_"` that keeps the startup status. A trap that changes `$_` leaves
  that value at the first prompt.
- A startup file that sets `set -e` and ends with a nonzero status exits the
  shell before the first prompt, because the bootstrap's `source` returns
  that status. A plain start keeps the shell. A `DEBUG` trap
  installed after startup on bash 4.4 or later also sees Huterm's final
  `PROMPT_COMMAND` entry.
- When readline redraws the whole prompt, as for Ctrl-L or a terminal resize,
  bash resends the prompt's markers, including the completion marker, so a
  completion marker can repeat without a new execution marker. In zsh, a
  `zle reset-prompt`, as async git prompts and transient prompts use, resends
  the prompt and input markers.
- Bash on distributions such as Debian, Ubuntu, and Arch reads its system
  bashrc, such as `/etc/bash.bashrc`, for every interactive start that has an
  rcfile. An emulated login shell therefore runs it once more than a plain
  login start: twice when `/etc/profile` also reads it, and once with
  `--noprofile`.
  Huterm starts non-login bash on Linux, which matches a plain start.
- Re-sourcing a bash startup file that replaces `PROMPT_COMMAND` removes
  Huterm's hook from that shell: the re-sourcing command gets an execution
  marker without a completion marker, and later prompts and commands get none.
  With `promptvars` off, the static `PS0` marker keeps marking execution
  without completion. A re-sourced file that keeps Huterm's entries, for
  example by adding to `PROMPT_COMMAND`, keeps integration, and Huterm
  decorates the file's new `PS1`. On bash 3.2, or with `promptvars` off, a
  re-sourced file that prepends to `PROMPT_COMMAND`, as in
  `PROMPT_COMMAND="history -a; $PROMPT_COMMAND"`, gives the next command's
  completion marker the prepended command's status for one cycle. In zsh,
  re-sourcing a `.zshrc` that assigns
  `precmd_functions` loses one prompt cycle: the re-sourcing command has no
  completion marker and the next prompt has no prompt markers. The next
  command restores Huterm's hooks.
- A nested bash that inherits an exported `PROMPT_COMMAND` keeps `$?` and `$_`
  through Huterm's inherited entries, but a later entry sees `PIPESTATUS`
  holding only `$?` after a failing command, and each inherited entry forks
  once after a status above 1. A preexec-style `DEBUG` trap there that does
  not skip `PROMPT_COMMAND` lines, as bash-preexec does, sees the entries as
  commands. With `promptvars` off in the parent, an exported `PS1` or `PS0`
  carries Huterm's static markers into nested shells; with `promptvars` off in
  the nested shell, an inherited decorated `PS1` shows Huterm's
  `${_huterm_bash_...-}` text.
- With bash-preexec 0.7 on bash older than 5.1, Huterm decorates the first
  prompt before bash-preexec installs itself, so that prompt has no Huterm
  markers when a `precmd` function rebuilds `PS1`.
- A standalone subshell command such as `( false )` has no execution or
  completion markers on bash 3.2. Bash does not run the parent's DEBUG trap
  for a subshell compound command. Enabling function tracing would change
  user traps and still leave the parent's completion state unchanged.
- Elvish emits A, C, D, and OSC 7, with no B marker because its asynchronous
  prompt renderer filters control sequences. Its guarded `use` keeps a broken
  module from stopping the rest of `rc.elv`.
- A triple-click on typed input stops at the input marker. In zsh, the
  selection also takes the prompt in these cases:
  - `prompt_percent` is unset, so the prompt has no input marker;
  - a framework's own markers, which Huterm leaves alone, omit the input
    marker;
  - the window is resized after a theme redrew its own prompt, because zsh
    redraws the theme's unmarked `PROMPT`; the next prompt is marked again;
  - a theme redraws with `builtin zle reset-prompt`, which skips Huterm's
    `zle` function, or a `zle` function defined by a startup file replaces it.
- Huterm's `zle` is a function. With a `ZERR` trap set, zsh does not apply
  `err_return` to a failing function, so a widget with both continues after a
  failing `zle` call. A trap that runs `return` just as Huterm marks a redrawn
  prompt can leave the marks in `PROMPT`. Later commands then get no execution
  or completion markers until the theme sets `PROMPT` again.
- An `/etc/zshenv` that changes `ZDOTDIR` before the stub can bypass loading.
  Startup files that replace `XDG_DATA_DIRS` before fish or nushell discover
  vendor files can also bypass it.
- Bash and zsh skip their OSC 133 stream for a prompt containing another
  framework's markers, including escaped spellings. A framework that prints
  markers from hooks cannot be detected. OSC 7 and `path` remain active.
- A missing bootstrap starts the original command and prints a core warning
  at most once per process.
  Body parse/load errors print one `huterm: shell integration failed to load:
  <path>` line and preserve startup. Arbitrary corruption of a bootstrap before
  its restoration code can prevent Bash's emulated startup or Zsh's restored
  startup directory. Packaging verifies that shipped files match the source
  at build time; runtime corruption is not detected.

Huterm's own directory reports use `kitty-shell-cwd://host/literal path`.
Spaces, `#`, `?`, and `%` stay literal. Reports containing control characters
are skipped rather than changed. Native fish and nushell may use `file://`.

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

Each pane of a split tab has a title bar with its title, Split Right, Split
Down, zoom, and close buttons, a drag handle, and a context menu. The split
buttons act on their own pane and focus the new pane. Narrow bars hide both
split buttons first, then zoom; close stays available. `title_bar_position`
puts the bars, and the focus accent described below, along the top or bottom
of their panes.

Set `title_bar_always_show = true` to show a title bar with one pane too. It
follows the same mode and position: visible bars take space from the terminal,
hover bars overlay it, and hidden bars stay hidden. A lone pane's bar has split
and close buttons; closing it closes the tab. It has no zoom button.

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

Three settings show which pane has focus while more than one pane is on
screen; each is off at 0. `focus_accent` draws a thin accent line along the
focused pane's edge where its title bar would be, at that opacity from 0 to 1,
and is off by default. It draws only under hover or hidden bars, and under
hover bars only until the focused pane's bar appears. `inactive_dim` dims every
other pane's terminal by laying the theme's background color over it, from 0
to 0.8, and defaults to 0.2. `inactive_dim_background` lays black over each
unfocused pane's terminal, including its background, from 0 to 0.5, and is
off by default. The two dimming settings work independently and stack when
both are on. Title bars stay as they are, and clicks, selections, scrolling,
links, and drops reach a dimmed pane as usual. All nine settings apply on
reload:

```toml
[panes]
title_bar = "visible" # visible, hover, or hidden
title_bar_always_show = false # also on a tab's only pane
title_bar_position = "top" # top or bottom
title_bar_hover_delay_ms = 75 # 0 to 2000
title_bar_fade_ms = 75 # 0 to 1000
title_bar_hide_delay_ms = 400 # 0 to 5000
focus_accent = 0.0 # 0 (off) to 1
inactive_dim = 0.2 # 0 (off) to 0.8
inactive_dim_background = 0.0 # 0 (off) to 0.5
```

### Sync and zoom indicators

A tab that syncs input, or shows a zoomed pane, says so with an indicator
on its panes and a mark in the tab bar. By default a syncing tab tints the
title bar of each pane typing reaches in the tab accent, and a zoomed tab
tints the zoomed pane's title bar in the theme's magenta, both at half
strength. A zoomed tab also shows a zoom mark before the tab's title. While
a single tab hides the tab bar, the window's title row shows both marks
instead. While a tab both syncs and shows a zoomed pane, typing reaches only
the zoomed pane, so the zoom look draws and the sync mark dims.
Each state has its own table:

```toml
[panes.sync_indicator]
style = "title_bar" # outline, dividers, glow, edge, title_bar, or off
width = 2 # 1 to 6 points
opacity = 0.5 # 0.1 to 1
pattern = "dashed" # solid, dashed, or dotted

[panes.zoom_indicator]
style = "title_bar" # outline, glow, edge, title_bar, or off
width = 2
opacity = 0.5
pattern = "solid"
```

`outline` draws one line around the panes and, for sync, along each
divider. `dividers` draws the dividers alone and is available only for sync;
a zoomed tab shows no dividers. `glow` draws a soft band inside the edge, and
along each divider for sync. `edge` draws one bar along the top of the panes.
`title_bar` tints the title bar of each pane typing reaches, or of the zoomed
pane, and shows a small chip in the pane's corner where it has no bar, as in a
tab of one pane, or its bar is hidden or has not appeared yet. `off` draws
no shape and keeps the marks. `width` sets the thickness of the lines,
`opacity` how strongly the shape draws, and `pattern` whether `outline`,
`dividers`, and `edge` lines are solid, dashed, or dotted. A table you write
only part of keeps its own defaults for the rest. Every key applies on
reload. The corners are square in fullscreen, in
a Quake window's drop-down, and on Linux, where the window's bottom corners
are square.

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

## Dragging between windows

A tab or pane dragged onto another Huterm window drops into its tab bar, its
title bar while the tab bar is hidden, or an edge of one of its panes.
Released outside every Huterm window, it opens in a new window as that
window's only tab. A window's only tab, and the only pane of a window's only
tab, stay where they are.

On macOS a small preview follows the pointer once the drag leaves its window:
the dragged title over another Huterm window, a miniature window where a
release opens a new one, and the title dimmed where a release does nothing.
`macos_drag_preview = false` turns it off. Without it, and on Linux, the source
window shows the outcome itself: a `↗ New window` pill after its tabs for a tab,
and the dragged pane dimmed with the pill in its middle. The setting applies on
reload, including to a drag under way. In Settings it is **Floating drag
preview** under Windows, among the advanced settings.

```toml
[window]
macos_drag_preview = true # ignored on Linux
```

## Initial window size

```toml
[window]
initial_columns = 100 # 10 through 1000
initial_rows = 32     # 3 through 1000
```

These values set the terminal grid in the first window and every new ordinary
window. Padding, tabs, title bars, and window decorations add to the grid's
size. The display's available area limits large windows, which then have a
smaller grid; the minimum window size still applies to small requests. On
Linux, the window manager handles any further work-area limits. Quake windows
keep the size their profiles specify.

Changes apply to windows opened afterwards. Reloading does not resize open
windows.

## Power frame limits

```toml
[window]
power_frame_limits = true
```

Terminal windows and the macOS floating drag preview draw at most about 30
frames per second while the system saves power, and at most about 60 under
serious or critical thermal pressure.
On macOS, Low Power Mode engages the first cap and the system's thermal state
the second. On Linux, power-profiles-daemon's `power-saver` profile engages the
first cap; Linux reports no thermal state, so the second never engages there.
Set `power_frame_limits = false` to turn both caps off.

In Low Power Mode on a 60 Hz display, sustained output drew 30 frames per
second with the caps on and 60 with them off, and typing a key could take up
to one 30 fps frame, about 33 ms, to show its echo. macOS did not throttle the
window itself with the caps off.

Settings lists it under Windows with the advanced settings. Changes apply to
windows opened afterwards, and to the drag preview from the next drag; open
windows keep the limits they opened with.

## Tab bar

Tabs can sit at the top, bottom, left, or right of the window, or in the title
bar. The title bar is the macOS default; Linux defaults to the top. These are
the `[window]` and `[tabs]` settings with their defaults:

```toml
#:schema https://github.com/jimeh/huterm/releases/latest/download/huterm.schema.json

[window]
opacity = 1.0
background_opacity = 1.0
foreground_opacity = 1.0
blur = 0 # 0 (off) to 64; macOS only
padding_x = 4.0
padding_y = 4.0
padding_balance = false
macos_fullscreen_mode = "non_native" # native or non_native; ignored on Linux
menu_button = true # the window menu button in the title bar or tab bar
session_button = true # the session control beside the menu button
shortcut_hints = false # key hints on the scroll pill and dialog buttons
macos_native_menus = true # native macOS menus; ignored on Linux
macos_drag_preview = true # floating drag preview; ignored on Linux
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
sidebar width. On macOS the sidebar fills the window height, with its rows
below the title bar. The title bar spans the terminal column in the terminal
background; traffic lights and the menu button stay at the window's corners.
Drag empty space in either the title bar or the strip above the sidebar rows
to move the window, or double-click it for the system title-bar action.
A title bar without tabs has a thin line above split panes, including while
zoomed, or above a visible top pane title bar. Hover title bars do not add it.

Drag a tab to reorder it within its window. The pointer shows a closed hand
from the press until release or cancellation. The preview stays in the tab bar
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

## Transparency

Windows can let the desktop show through. These are the `[window]` settings
with their defaults; in Settings they are under Appearance > Transparency:

```toml
[window]
opacity = 1.0            # 0.1 to 1: fades the whole window evenly
background_opacity = 1.0 # 0 to 1: the terminal background and chrome bars
foreground_opacity = 1.0 # 0.1 to 1: terminal text, labelled Text opacity
blur = 0                 # 0 (off) to 64: blur radius, macOS only
```

`opacity` fades everything in the window together, chrome, text, cursor and
selection included. `background_opacity` sets how much of the terminal's
default background shows, together with the padding, the tab bar, the title
row, the tab sidebar and pane title bars; their text, icons and dividers stay
solid. `foreground_opacity` fades terminal text, box-drawing and powerline
characters, underlines, strikethrough and link underlines. `opacity`
multiplies the other two: opacity 0.7 with background opacity 0.6 leaves the
background at 0.42. All four apply to open windows, Quake windows included, as
soon as the file reloads, without restarting shells or losing scrollback. They
stay in effect in fullscreen.

Only the default background turns transparent, including a default background
that a program changed with OSC 11. Background colors that programs set, such
as an editor's status line, and inverse video stay solid, even when their color
matches the theme background. The cursor, the selection and selected text also
stay solid apart from `opacity`. Menus, dialogs, notices, the command palette,
pane title bars revealed on hover, the tab bar revealed in fullscreen, and the
Settings window stay opaque so they remain readable over the terminal.

A few things look slightly different below full opacity. Unfocused panes dim
their colors rather than drawing a shade over them, so emoji in them are not
dimmed, and emoji keep full strength at reduced text opacity. Where box-drawing
characters overlap, reduced text opacity can show the overlap. The visual bell's
flash briefly looks more solid.

`blur` blurs what shows through the window whenever the window is
translucent, that is while `opacity` or `background_opacity` is below 1. It
is the window server's blur radius in points, from 0 (off) to 64, and
changes apply to open windows at once. An opaque window never blurs.

The window server fades its blur together with the whole window, so while a
window blurs Huterm keeps the window itself fully opaque and fades its
contents by `opacity` instead. Every part of the window fades evenly, but
where translucent layers overlap, such as a program's background color over
the default background or text over a translucent bar, the overlap shows a
little more solid than it does without blur. Menus, tooltips, and other
popups stay solid in blur mode. With `blur = 0`, `opacity` fades the whole
window as one, as before. The Quake fade animates the whole window in both
modes.

Linux has no blur: Huterm logs a warning when the file or a Quake profile
sets a radius, and Settings and the transparency panel show no blur control,
except that Settings shows a row the file sets.

### Per-window transparency

View > Window Transparency… (`window_transparency` in the palette, with no
default binding) opens a panel at the top right of the terminal area that
changes these four values for the focused window alone. Each row shows where
its value comes from: Default (Appearance > Transparency), Profile (the
window's Quake profile), or This window, an override with a dot. Dragging or
stepping a slider overrides that value at once, Reset puts back the value
beneath it, Reset All clears every override, and Edit Defaults in Settings…
opens Settings at Appearance > Transparency. Escape, the command again, or a
click outside the panel closes it. Each value is layered on its own: the
defaults, then the window's Quake profile, then the window's override. A
later change to a default reaches every window that does not override that
value. Overrides last as long as the window and are not saved; a tab torn
off into a new window starts without any. The [keybindings
reference](keybindings.md#window-transparency-panel) lists the panel's keys.

Without transparency support the panel's sliders are disabled and the panel
says why.

On Linux, transparency needs an X11 compositing manager and a 32-bit color
visual. Without either, windows stay opaque, Huterm logs which one is missing,
and Settings says so under Opacity, Background opacity and Text opacity. Huterm
checks again when a window opens and when the file reloads. A transparent
window on Linux draws text without subpixel antialiasing, and some graphics
drivers can keep a window opaque in a way Huterm cannot detect.

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
available through the palette and shortcuts.

`session_button` shows the session control just before the menu button: a
layers icon, the window's session name, and a chevron. It opens the session
menu, which switches, creates, renames, and closes sessions and opens one in
a new window. It follows the menu button's placement one slot further in, so
the menu button keeps the rightmost slot, and takes the button's slot when
`menu_button = false`. In a left or right column without a title bar it shows
only its icon in the new-tab row. Its name truncates at 168 points and in
narrow hosts, and below about 345 points of title bar it shows only the icon;
the tooltip and accessible name keep the full name. It hides with an
auto-hidden bar. `session_button = false` removes it; `open_session_menu` and
the session commands stay available. The session menu follows
`macos_native_menus` like the other menus; a native menu shows each session's
tab count and tags as a subtitle on macOS 14.4 and later. Right-clicking empty tab-bar
space also opens the window menu, except in Huterm's own Linux title row,
where it opens the window manager's menu.

Menus and the command palette show each command's shortcut: in symbols on macOS
(`⇧⌘P`) and in words on Linux (`Ctrl+Shift+P`), with Return, Tab, Backspace,
and the arrows drawn as icons on both. The scroll pill, the close
confirmation's buttons, the About panel, and the Licenses view show key hints
only with `shortcut_hints = true`.

On macOS the window, tab, pane, terminal, and session menus, and a text field's
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
- **Sync Pane Input**, worded **Stop Syncing Pane Input** while the tab
  syncs, for a split tab or one that syncs. It toggles that tab's input
  sync, with its shortcut shown only on the active tab.

The tab menu does not open during a tab drag or while a close confirmation is
showing. It uses the same keys, dismissal, and focus return as the window menu.

Right-clicking a pane's title bar focuses the pane and opens its context
menu:

- **Rename Pane…**, which sets the name the title bar shows in place of the
  pane's title. A blank name restores the title.
- **Copy Directory Path**, disabled with "Directory unknown" until the pane
  reports a working directory.
- **Split Right** and **Split Down**.
- **Zoom Pane**, or **Unzoom Pane** while the pane is zoomed.
- **Sync Pane Input**, or **Stop Syncing Pane Input** while the tab syncs.
  With one pane, this row appears only while syncing.
- **Move Pane to New Tab**, **Move Pane to New Window**, and **Move Pane to
  Another Pane…**.
- **Auto-hide Title Bars**, checked in hover mode. Picking it saves `title_bar`
  as `hover` or `visible` in your configuration file and reloads every window,
  even with `watch_config = false`. A failed save shows a notice and keeps the
  current mode.
- **Close Pane**.

With one pane, **Zoom Pane** and **Move Pane to New Tab** are disabled with
"Tab has one pane". **Move Pane to New Window** is disabled only for the pane
of a window's only tab, with "The pane is already alone in its window."
Dragging a lone pane's bar onto another tab's label switches to that tab after
a moment, and dropping it on a pane's edge there joins that tab as a split.
The drag does not reorder tabs.

While the tab bar is hidden, drop a dragged pane on empty space in the window's
title bar to move it into a new tab. The source tab must have more than one
pane. This works on macOS and with Huterm's Linux title bar; fullscreen and
Quake windows have no title bar for this drop.

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
# opacity = 0.9             # optional; unset uses [window] opacity
# background_opacity = 0.8  # optional; unset uses [window] background_opacity
# foreground_opacity = 1.0  # optional; unset uses [window] foreground_opacity
# blur = 20                 # optional, macOS only; unset uses [window] blur

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

A profile's `opacity`, `background_opacity`, `foreground_opacity`, and
`blur` override the [`[window]` transparency](#transparency) values for its
window, each on its own, and a window's own overrides from the transparency
panel come on top. An unset key uses the `[window]` value; Settings shows it
in the profile's Transparency card as, for example, "Uses Appearance >
Transparency (60%).", and Reset there removes the key so the profile follows
the default again. A change to these keys applies to the window at once
without showing, hiding, or moving it. The window keeps its profile's values
while shown as a regular window, and the Quake fade ends at the window's
resolved opacity.

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
resolution keeps it. A move made after a display was connected or disconnected,
or the primary display changed, during the same regular presentation does not
count, since the system may have moved the window. A window hidden in regular
presentation shows again where it was. Explicit IDs use CoreGraphics display
UUIDs on macOS and RandR monitor names on X11.

Global bindings use a single keystroke, no `when`, and only the three quake
commands. They must not conflict with the first keystroke of a local binding you
wrote. One that takes the key of a default binding, such as
`toggle_pane_input_sync`'s, is only a warning: the shortcut gets the key while
it is registered, and the default still works in Huterm when it is not. Disable
or unbind that default to silence the warning. An unsupported key or an invalid
binding is a configuration error, and a rejected reload keeps the previous
configuration and shortcuts. A shortcut another application already holds is
only a warning: the rest of the configuration applies, the other shortcuts
register, and the next reload, including the one a save starts, tries it again.
Global shortcuts work on macOS and native X11 sessions, but not on Wayland or
XWayland, where configuring one warns the same way.

## Fonts and themes

```toml
[font]
family = "Menlo" # defaults to Menlo on macOS and DejaVu Sans Mono on Linux
size = 14.0      # 6 through 96
# thin, extra_light, light, regular, medium, semibold, bold, extra_bold, black
weight = "regular"
ui_family = ""   # empty uses the system interface font
ui_size = 13.0   # 10 through 16
nerd_font_symbols = true # draw Nerd Font icons from the bundled font
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

Huterm bundles DejaVu Sans Mono 2.37 (Book, Bold, Oblique, and Bold Oblique).
Linux uses it as the default terminal font and for monospace interface text.
macOS keeps Menlo as its default. An unavailable terminal family falls back to
that platform default; Settings and the startup warning name the family that
draws. If the default font fails to load, they report it as unavailable too.

`family = "monospace"` is a font alias. Huterm draws it with the platform
default, including in configuration files generated by earlier versions, and
Settings explains the alias without calling it uninstalled. Its weight selector
lists the drawn family's weights and applies `weight` normally. Resolving the
alias through fontconfig is deferred.

Huterm also bundles Symbols Nerd Font from Nerd Fonts 3.5.1, so prompt and
file-listing icons from tools such as Starship, Powerlevel10k, eza, and Neovim
draw with any terminal family. A Private Use Area icon, or one of the IEC
power symbols, draws from it when the terminal family has no glyph of its
own. A Nerd Font patched family therefore keeps its own icons. Each icon is
sized and aligned in its cell by the Nerd Fonts patcher's rule for it, as a
patched "Nerd Font Mono" family draws it, using a port of Ghostty's rules: an
icon fills one cell, or both columns of a wide cell, and tall icons such as
the Git branch can use the full line height. A few separators may overlap
the next cell slightly by design. Box drawing, block elements, and the
Powerline separators that Huterm draws itself are unchanged. The icon font is
not offered as a terminal or interface family. Set `nerd_font_symbols = false`,
or turn off **Built-in Nerd Font icons** in Settings, to leave those code
points to the terminal font and the system's font fallback; the change applies
to open terminals immediately.

On macOS the bundled DejaVu faces draw even when the system has that family
installed. On Linux an installed DejaVu face with matching properties takes
precedence, and bundled faces cover missing weights and styles. Settings lists
the family once. A different installed version on Linux may therefore draw
instead of the bundled 2.37 copy.

`weight` sets the terminal font's weight and accepts the names above or
integers from 100 through 900 in steps of 100. Regular is 400. Bold terminal
text asks for at least Bold (700), or 300 more than the configured weight,
capped at Black (900). For example, Medium (500) asks for Extra bold (800) for
bold text. Both draw in the nearest weight the family has, so when it has no
heavier face, bold text looks like the rest.
These weights apply on reload; the interface keeps its own normal and
semibold weights.

Huterm uses the nearest face the family provides, without synthesizing weights.
Settings offers the family's available weights and says which weight terminals
use when the configured weight is missing, including for a family with one
weight. A family with one weight has a
disabled weight selector. If enumeration fails, all nine weights remain
available. On macOS, a weight installed into an already loaded family may appear
in Settings before the terminal can draw it; restart Huterm to use it.

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

A bell in a window that is not active also asks for attention without taking
focus: on macOS the Dock icon bounces once, only while Huterm is in the
background, and on Linux under X11 the window is marked urgent until you
focus it. Wayland has no such request, so nothing happens there. Turn it
off with `attention = false`:

```toml
[terminal.bell]
attention = false
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

## Notifications and progress

Programs can ask for a desktop notification with OSC 9 (`ESC ] 9 ; text
BEL`) or OSC 777 (`ESC ] 777 ; notify ; title ; text BEL`). Huterm shows
each one once, in the window you used most recently that shows the terminal:

- While Huterm is in the background, it is a system notification, titled with
  the tab's name or with the program's title and the tab's name. Clicking it
  brings Huterm forward with that terminal's window, tab, and pane, summoning a
  hidden Quake window and clearing a zoom that hid the pane.
- While Huterm is in front, it is a notice in the active window, or in the
  terminal's own window while Settings is in front. When the terminal is in
  another tab or window, or behind a zoomed pane, the notice offers
  **Show Tab**. Each tab keeps only its newest notification notice.
- A tab that sent a notification you have not seen shows the bell mark until
  you view it.

If system notifications cannot show, because macOS notification permission
is off for Huterm, Huterm runs outside an app bundle (as `mise run dev`
does), or no notification service answers on Linux, the notice appears in
the terminal's window instead and the window asks for attention as a bell
does. On macOS, the first notification asks for permission; change it later
in System Settings > Notifications.

Notification text is plain text. Huterm removes control and bidirectional
formatting characters, keeps at most 128 characters of a title and 512 of
the text, and never turns text into a link or button. Each terminal may send
4 notifications at once and then one every 3 seconds, and all terminals
together 8 at once and then one a second; extra notifications are dropped.
Ghostty drops OSC 9 and OSC 777 sequences longer than 2,048 bytes. Kitty's
OSC 99 notifications are not supported.

Programs report progress with OSC 9;4 (`ESC ] 9 ; 4 ; state ; percent
BEL`): state 1 sets a percentage, 2 reports an error, 3 progress without a
percentage, 4 a pause, and 0 removes it. A ring replaces the running dot
before the tab's and pane's title, and a thin bar runs across the top of the
pane. Paused progress is yellow and errors are red. Progress clears when the
program removes it, resets the terminal, or exits, or after 15 seconds
without a report. It is never saved with sessions.

```toml
[terminal]
notifications = "deny" # "allow" or "deny"
progress_bar = false # keep the tab's ring, drop the pane bar
```

Reload applies both to open terminals; `deny` also cancels notifications
that have not been shown yet. As with `clipboard_write`, an explicit `deny`
survives a startup fallback caused by an unrelated settings error.

On Linux, at most 16 system notifications wait for a click at once. Later ones
still show while those remain open, but clicking them does nothing.

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
