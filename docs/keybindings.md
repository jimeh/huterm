# Commands and keybindings

Every menu item, shortcut, and palette entry runs a command from one catalog.
Each command has a stable ID, a scope, and named typed arguments. Keybindings
live in the same file as the other settings; see
[Configuration](configuration.md) for its location and reload.

The Keybindings section of Settings edits them in place. Each command row
expands to its shortcuts: click a key, or press Space on it, then press the new
shortcut. A second key within a second makes a chord, Return saves at once,
and Escape cancels. Holding a key records it once. The recorder takes keys
that are bound elsewhere, so `cmd-w` records instead of closing the window.
When the new shortcut would take its key from another command with the same
`when`, Settings asks first. Reassign removes your own entries that bind that
key under the same `when` to other commands, and Cancel leaves both commands as
they were. A recorded shortcut moves to the end of the file when a later entry
with the same key and `when`, or a later `unbind` of the key, would otherwise
take it back. Each
shortcut also has a `when` field that completes context names and checks
the condition before saving, its argument values, and a switch. Changing or
switching off a default writes a `disabled = true` entry for exactly that
shortcut, plus the changed binding; Restore defaults removes the command's
entries. Keys the operating system or a global shortcut takes first cannot be
recorded.

## Command palette

Open the command palette with Cmd+Shift+P on macOS or Ctrl+Shift+P on Linux, or,
on macOS, choose **View > Open Command Palette**. Search is fuzzy across command
titles, IDs, and descriptions, so `tfs` finds Toggle Fullscreen. Results rank by
match quality, then this window's recent commands, then how often the app has
run each command, then catalog order. Unavailable commands stay visible with the
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

## Keybindings

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

# Keep cmd-h for the shell; Hide stays in the menu.
[[keybinding]]
key = "cmd-h"
command = "hide"
disabled = true
```

The fields are:

- `key` (required): the keystroke or chord.
- `command` (required): a command ID from the [table below](#commands), or
  the reserved `unbind` described below.
- `args`: a table of named arguments. Arrays are rejected.
- `when`: an optional context predicate.
- `description`: an optional label for the palette; blank text is rejected.
  Without it, the binding is labelled with the command title and its
  arguments, such as `Select Tab (index: 3)`.
- `disabled`: `true` turns off earlier bindings instead of adding one, as
  described below. `false` is the same as leaving it out.

Unknown fields are rejected, as everywhere in the config.

`unbind` is a reserved command that removes every earlier binding for the key,
including defaults, regardless of `when`. It takes no `args`, `when`, or
`disabled`. Unbinding a key with no binding does nothing.

`disabled = true` is narrower. It removes each earlier binding, default or
user, with the same key, the same command, and the same `when` combined with
the command's own context, and binds nothing itself. Bindings on that key for
other commands or other `when` predicates stay. With `args`, an earlier
binding must also have the same arguments; without `args`, any arguments
match. So `cmd-h` with `command = "hide"` and `disabled = true` frees Cmd+H
from Hide but keeps a `cmd-h` binding you added for another command, and the
key reaches the shell unless something else still claims it. The command and
its arguments are still checked, so a typo fails like any other bad entry. A
disabled entry that matches nothing does nothing, so one file can disable a
macOS default and still load on Linux. Like every entry, it affects only
entries before it: a later entry can bind the key again, and a later `unbind`
still removes what remains. On a global shortcut, `disabled = true` keeps the
entry in the file without registering it.

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
| `split` | That terminal's tab has two or more panes, whether or not one is zoomed. |
| `pane_zoomed` | The window shows that terminal's pane zoomed: the window's own zoom under `[panes] zoom = "view"`, the tab's under `"shared"`. |
| `has_pane_left`, `has_pane_right`, `has_pane_up`, `has_pane_down` | `focus_pane_left`, or the command for that side, has a pane to move to from that terminal. They follow the layout, so zoom does not change them. |
| `Workspace` | Always, on the window's root. |
| `confirming` | A close confirmation, the About panel, or a Settings dialog such as Delete profile's is open. |
| `menu` | The window menu has keyboard focus. |
| `notices` | A notice has keyboard focus (after `focus_notices`). |
| `reordering` | A tab drag is in progress. |
| `fullscreen` | The window has finished entering native or non-native fullscreen. Pending entry alone does not match. |
| `palette` | The window owns an open command palette. |
| `Palette` | The command palette or the About panel's Licenses view is open and has focus. |
| `TextField` | A text field has focus: a Settings field or dialog field, the palette's field, or the Licenses search. The Licenses buttons carry it too, since text edits there go to the search. In the palette and Licenses it sits on the same element as `Palette`. |
| `Settings` | The Settings window has focus. |
| `SettingsControl` | A Settings switch, select, segmented control, slider, modifier chip, theme card, button, or section has focus. |
| `SettingsGroup` | A Settings section, theme card, or modifier chip has focus. It sits inside that item's `SettingsControl`, so a binding under it matches deeper and wins over a `SettingsControl` binding on the same key. |
| `control` | `slider` while a Settings slider has focus, so `SettingsControl && control == slider` matches sliders alone. `button` while a dialog button has focus, on the same element as its `confirming`, so `confirming && control == button` matches the button and never a text field in the dialog. |

Most contexts are bare words for the state of the view or window, such as
`selection` and `pane_zoomed`. The `has_` prefix marks a context that says
something exists elsewhere, such as a pane to the left.

The pane contexts let a key move between panes where it can and reach the
shell where it cannot, so tmux or another multiplexer running inside a pane
still receives it. These bindings move focus with Alt and `i`, `j`, `k`, or
`l`, and send the key to the pane at the tab's edge and in a tab with one
pane:

```toml
[[keybinding]]
key = "alt-i"
command = "focus_pane_up"
when = "Terminal && has_pane_up"

[[keybinding]]
key = "alt-j"
command = "focus_pane_left"
when = "Terminal && has_pane_left"

[[keybinding]]
key = "alt-k"
command = "focus_pane_down"
when = "Terminal && has_pane_down"

[[keybinding]]
key = "alt-l"
command = "focus_pane_right"
when = "Terminal && has_pane_right"
```

Focus commands unzoom first, so these keys still move focus from a zoomed
pane. To send them to a zoomed pane instead, add `!pane_zoomed`, as in
`when = "Terminal && has_pane_left && !pane_zoomed"`. On macOS, set
[`macos_option_as_alt`](configuration.md#option-and-alt-as-meta) so the keys
that reach the shell arrive as Meta chords.

Key contexts update when the window draws its next frame. A key pressed
between a layout change and that frame matches the previous contexts.

## Commands

A command's scope says who runs it. Runtime commands run in the core against
the canonical session structure. The `pane`, `tab`, `workspace`, and
`session` arguments cannot be written in the config for any command. The
palette prompts for them where a command needs one, and omitted targets
default to the invoking window's active tab, workspace, or session.
Focused commands run in whatever has keyboard focus: the palette, the
Licenses view, or a text field's owner.
The `palette_*` commands imply the `Palette` context and the `text_*`
commands imply `TextField`, and dialog, menu, and notice commands imply
`confirming`, `menu`, and `notices`. Settings commands imply `Settings`,
`SettingsControl`, or `SettingsGroup`. User bindings for them need no `when`.
A `when` adds to the implied context on the same element, so a `text_*`
binding with `when = "Palette"` matches only in the palette and Licenses,
where both contexts sit together, and no longer in Settings fields. Huterm
reports such a binding as a keybinding warning; drop `Palette` from its
`when` to bind it in every text field. A custom key for
`palette_select_next`, `palette_select_previous`, `palette_page_down`, or
`palette_page_up` no longer steps a Settings number field or its
suggestions; bind it to `text_next`, `text_previous`, `text_page_down`, or
`text_page_up` for that.

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
| `open_settings` | Application | Opens the Settings window, or brings it forward. |
| `open_config_file` | Application | Opens `config.toml` in the system editor, creating it first. |
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
| `split_right` / `split_down` | Window | Splits the focused pane; the new pane opens beside or below it and takes focus. |
| `split_left` / `split_up` | Window | Splits the focused pane; the new pane opens before or above it and takes focus. |
| `close_pane` | Window | Closes the focused pane, asking first when it runs a job. Closing a tab's only pane closes the tab. |
| `focus_next_pane` / `focus_previous_pane` | Window | Focuses panes in layout order, wrapping. |
| `focus_pane_left` / `focus_pane_right` / `focus_pane_up` / `focus_pane_down` | Window | Focuses the adjacent pane in that direction; does nothing at the tab's edge. |
| `resize_pane_left` / `resize_pane_right` / `resize_pane_up` / `resize_pane_down` | Window | Optional `amount` in cells (1 to 100), default 5. Moves the focused pane's edge that way, keeping every pane at least 4 columns by 2 rows; does nothing without a neighbour on that side. |
| `toggle_pane_zoom` | Window | Shows the focused pane across its tab, or every pane again. Focus, split, and resize commands unzoom first. |
| `swap_pane_next` / `swap_pane_previous` | Window | Swaps the focused pane with the next or previous pane in layout order, wrapping. Each pane keeps its terminal; the two trade places. |
| `swap_pane_left` / `swap_pane_right` / `swap_pane_up` / `swap_pane_down` | Window | Swaps the focused pane with the pane `focus_pane_*` would move to; does nothing at the tab's edge. |
| `move_pane_next` / `move_pane_previous` | Window | Moves the focused pane one place forward or back in layout order, wrapping. |
| `move_pane_left` / `move_pane_right` / `move_pane_up` / `move_pane_down` | Window | Moves the focused pane past the adjacent pane on that side; does nothing at the tab's edge. |
| `move_pane_to_pane` | Window | `pane` and `side` (`left`, `right`, `up`, or `down`). Moves the focused pane beside a pane in any tab, workspace, or session. A binding may set `side`; the palette prompts for the pane, and for the side when the binding omits it. |
| `move_pane_to_new_tab` | Window | Optional `workspace`; defaults to the window's, where the new tab opens after the pane's tab. A tab's only pane moves its whole tab to another workspace. |
| `move_tab_to_pane` | Window | `pane` and `side`; `tab` defaults to the active tab. Moves the whole tab beside a pane of another tab, keeping its layout. A binding may set `side`; the palette prompts for the pane, and for the side when the binding omits it. |
| `arrange_panes` | Window | `layout`: `columns`, `rows`, `main_left`, `main_top`, `main_centered`, `tiled`, or a name from `[panes.layouts]` (see [pane layouts](#pane-layouts)). Rearranges the active tab's panes into that layout, keeping every terminal and the pane order. A binding may set `layout`; the palette and the Arrange Panes… menu items prompt for it. |
| `next_pane_layout` / `previous_pane_layout` | Window | Arranges the active tab's panes into the next or previous layout of the cycle, skipping a layout that does not fit the window or would change nothing. |
| `balance_panes` | Window | Gives every split of the active tab equal sizes, keeping its shape. |
| `undo` | Window | Undoes this window's last pane swap, move, extraction, tab drop, arrangement, or balance, putting the same panes and terminals back where they were, with the earlier divider positions. A tab the change removed comes back with its name, at its old position. Renames, reorders, and zooms made since survive. Splits, closes, resizes, and zoom are not recorded, and only the last change can be undone. Once a tab the change touched has another shape, or has moved to another session, the change is dropped and undo does nothing; if the runtime refuses an undo the window could not yet see was stale, a notice says so. With nothing to undo, or while the window is busy, the key does nothing. |
| `redo` | Window | Reapplies the change `undo` last undid. A new layout change replaces it. |
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
| `rename_pane` | Runtime | `name`; `pane` defaults to the active tab's focused pane. |
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
| `settings_search` | Window | Bound only while `Settings`. |
| `settings_focus_next` | Window | Bound only while `Settings`. |
| `settings_focus_previous` | Window | Bound only while `Settings`. |
| `settings_toggle` | Window | Bound only while `SettingsControl`. Toggles a switch or modifier chip, opens a select, presses a button, or shows the focused section. |
| `settings_increase` | Window | Bound only while `SettingsControl`. Steps the focused control; a switch, section, theme card, or modifier chip ignores it. |
| `settings_decrease` | Window | Bound only while `SettingsControl`. Steps the focused control back; a switch, section, theme card, or modifier chip ignores it. |
| `settings_page_up` | Window | Bound only while `SettingsControl`. Steps the focused slider up one page, a tenth of its range; other controls ignore it. |
| `settings_page_down` | Window | Bound only while `SettingsControl`. Steps the focused slider down one page, a tenth of its range; other controls ignore it. |
| `settings_first` | Window | Bound only while `SettingsControl`. Moves the focused slider to its minimum; other controls ignore it. |
| `settings_last` | Window | Bound only while `SettingsControl`. Moves the focused slider to its maximum; other controls ignore it. |
| `settings_next` | Window | Bound only while `SettingsGroup`. Moves to the next section, theme, or modifier chip of the focused group, wrapping from the last to the first; a section or theme it reaches is shown or chosen. Other controls ignore it. |
| `settings_previous` | Window | Bound only while `SettingsGroup`. Moves to the previous section, theme, or modifier chip of the focused group, wrapping from the first to the last; a section or theme it reaches is shown or chosen. Other controls ignore it. |
| `settings_reset` | Window | Bound only while `SettingsControl`. |
| `palette_select_next` | Focused | |
| `palette_select_previous` | Focused | |
| `palette_page_down` | Focused | |
| `palette_page_up` | Focused | |
| `palette_confirm` | Focused | |
| `palette_back` | Focused | |
| `palette_pop` | Focused | |
| `palette_expand` | Focused | |
| `palette_next_slot` | Focused | |
| `palette_previous_slot` | Focused | |
| `text_confirm` | Focused | Saves a Settings field or submits its dialog. The palette and Licenses leave it to `palette_confirm`. |
| `text_cancel` | Focused | Puts back a Settings field's value or cancels its dialog. The palette and Licenses leave it to `palette_back`. |
| `text_next` | Focused | Highlights the next suggestion, or steps a Settings number field down. The palette and Licenses leave it to `palette_select_next`. |
| `text_previous` | Focused | Highlights the previous suggestion, or steps a Settings number field up. The palette and Licenses leave it to `palette_select_previous`. |
| `text_page_down` | Focused | Pages suggestions or a Settings number field down. The palette and Licenses leave it to `palette_page_down`. |
| `text_page_up` | Focused | Pages suggestions or a Settings number field up. The palette and Licenses leave it to `palette_page_up`. |
| `text_delete_backward` | Focused | |
| `text_delete_forward` | Focused | |
| `text_delete_word_backward` | Focused | |
| `text_delete_word_forward` | Focused | |
| `text_delete_line_start` | Focused | |
| `text_move_left` | Focused | Optional `select` extends the selection instead. |
| `text_move_right` | Focused | Optional `select`. |
| `text_move_word_left` | Focused | Optional `select`. |
| `text_move_word_right` | Focused | Optional `select`. |
| `text_line_start` | Focused | Optional `select`. |
| `text_line_end` | Focused | Optional `select`. |
| `text_select_all` | Focused | |
| `text_cut` | Focused | |
| `text_copy` | Focused | |
| `text_paste` | Focused | |

## Default bindings

Commands without a default binding are available through the palette, the
menus, or your own bindings.

### Application, window, and terminal

| Command | macOS | Linux |
| --- | --- | --- |
| `new_window` | `cmd-n` | `ctrl-shift-n` |
| `open_command_palette` | `cmd-shift-p` | `ctrl-shift-p` |
| `new_tab` | `cmd-t` | `ctrl-shift-t` |
| `close_pane` | `cmd-w` | `ctrl-shift-w` |
| `split_right` / `split_down` | `cmd-d` / `cmd-shift-d` | `ctrl-shift-o` / `ctrl-shift-e` |
| `focus_next_pane` / `focus_previous_pane` | `cmd-]` / `cmd-[` | `ctrl-super-]` / `ctrl-super-[` |
| `focus_pane_left` and the other directions | `cmd-alt-` plus an arrow | `ctrl-alt-` plus an arrow |
| `resize_pane_left` and the other directions | `cmd-ctrl-` plus an arrow | `ctrl-super-shift-` plus an arrow |
| `toggle_pane_zoom` | `cmd-shift-enter` | `ctrl-shift-enter` |
| `next_pane_layout` / `previous_pane_layout` | `cmd-ctrl-]` / `cmd-ctrl-[` | `ctrl-super-alt-]` / `ctrl-super-alt-[` |
| `balance_panes` | `cmd-ctrl-=` | `ctrl-super-=` |
| `close_window` | `cmd-shift-w` | `ctrl-shift-q` |
| `next_tab` / `previous_tab` | `ctrl-tab` / `ctrl-shift-tab` | `ctrl-tab` / `ctrl-shift-tab` |
| `select_tab` 1 to 9 | `cmd-1` to `cmd-9` | `alt-1` to `alt-9` |
| `copy` / `paste` | `cmd-c` / `cmd-v` | `ctrl-shift-c` / `ctrl-shift-v` |
| `undo` / `redo` | `cmd-z` / `cmd-shift-z` | `ctrl-shift-z` / `ctrl-shift-y` |
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

The undo keys never reach the terminal, even with nothing to undo. On Linux,
plain `ctrl-z` stays with the shell for job control. Legacy terminal encoding
sends `ctrl-shift-z` to programs as plain `ctrl-z`, so taking it hides nothing
a shell relies on.

### Pane layouts

`arrange_panes` takes one of six built-in layouts, or a layout you define in
[the configuration file](configuration.md#pane-layouts). Layouts fill their
places with the tab's panes in layout order, so no layout reorders them. Swap
or move a pane first to make it the main pane. In Main Centered the main pane
is pane `ceil((n - 1) / 2) + 1` of `n`, so it moves as panes are added: the
second of two or three, the third of four or five.

| Layout | Name | Arrangement |
| --- | --- | --- |
| Equal Columns | `columns` | Every pane side by side, at equal widths. |
| Equal Rows | `rows` | Every pane stacked, at equal heights. |
| Main Left | `main_left` | The first pane on the left at 60 %, the others stacked on the right. |
| Main Top | `main_top` | The first pane on top at 60 %, the others side by side below. |
| Main Centered | `main_centered` | The middle pane in the centre at 50 %, the panes before it stacked on the left and the panes after it stacked on the right. With two panes the right stack is empty, and the panes take one third and two thirds. |
| Tiled | `tiled` | A grid of `ceil(sqrt(n))` columns, filled row by row; the last row's panes share its width. |

Bind a layout by name:

```toml
[[keybinding]]
key = "cmd-alt-1"
command = "arrange_panes"
args = { layout = "main_left" }

[[keybinding]]
key = "cmd-alt-2"
command = "arrange_panes"
args = { layout = "dev" } # from [panes.layouts]
```

The cycle visits the layouts in the table's order, or the order of
`[panes] layout_cycle`, which can add your own layouts and leave built-ins out.
It continues from the layout the tab matches, so resizing a divider keeps the
place, and after a split or a move that matches none it starts from the
beginning. Layouts and balancing keep every pane at least 4 columns by 2 rows in
the window that runs them, moving sizes from their ideal shares only as far as
that needs. A layout that cannot fit is refused with a notice, or skipped by the
cycle. Balancing never refuses: in a window too small for the tab's panes, they
share the space in proportion to what each needs. Each change is one undo step,
a zoomed tab unzooms, and a change that would leave the tab as it is does
nothing. The layout keys never reach the terminal.

No GNOME (Mutter, Shell, or settings daemon) or KDE KWin default uses the
Linux layout or balance keys. The nearest, GNOME's magnifier at `alt-super-=`
and KWin's zoom at `super-=`, need other modifiers. KWin binds
`ctrl-super-shift-` plus an arrow to moving a window to another desktop, which
takes the Linux resize keys there; rebind `resize_pane_left` and the other
directions if you use them.

### Palette

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

### Text fields

These bind under `TextField`, in every text field. The field verbs
(`text_confirm`, `text_cancel`, `text_next`, `text_previous`, `text_page_down`,
`text_page_up`) mean what the field's owner makes of them. The palette's field
and the Licenses view carry `Palette` beside `TextField` and leave the verbs
unhandled, so the key goes on to the palette's binding for it: Enter still
runs `palette_confirm` there, however the two bindings are ordered. Tab and
Shift-Tab are not bound here, so they reach `settings_focus_next`, a dialog's
`dialog_focus_next`, or `palette_expand`.

A right-click on a text field opens its menu: Cut, Copy, Paste, and Select
All, with the shortcuts the field's bindings give them. On macOS it is a
native menu, as the other context menus are, unless
`[window] macos_native_menus = false`. Cut and Copy are
disabled with nothing selected, Cut and Paste in a read-only field, and Select
All in an empty one. A right-click on the selection keeps it; anywhere else it
moves the caret there first, as on macOS. On macOS the Edit menu's Cut is
`text_cut`, and its Copy, Paste, and Select All act on a focused field as
`text_copy`, `text_paste`, and `text_select_all`; over a terminal, Cut is
disabled.

| Command | macOS | Linux |
| --- | --- | --- |
| `text_confirm` | `enter` | `enter` |
| `text_cancel` | `escape` | `escape` |
| `text_next` | `down`, `ctrl-n` | `down`, `ctrl-n` |
| `text_previous` | `up`, `ctrl-p` | `up`, `ctrl-p` |
| `text_page_down` | `pagedown` | `pagedown` |
| `text_page_up` | `pageup` | `pageup` |
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
| `text_cut` | `cmd-x` | `ctrl-x`, `ctrl-shift-x` |
| `text_copy` | `cmd-c` | `ctrl-c`, `ctrl-shift-c` |
| `text_paste` | `cmd-v` | `ctrl-v`, `ctrl-shift-v` |

### Dialog

These bind only while a close confirmation, the About panel, or a Settings
dialog is showing (`confirming`), so they take nothing from terminal input. The
primary button has focus when the dialog opens, and a repeated close shortcut
never confirms. `dialog_confirm` presses the focused button, so Enter or Space
on Cancel cancels. Its Space default carries `when = "control == button"`, so
Space presses only a focused dialog button and still types a space in a
dialog's text field. In the About panel, the focus commands move between its
Licenses, Copy Details, and Close buttons, `dialog_confirm` presses the focused
one, and `dialog_cancel` closes it. Focus starts on Close.

In Settings, Delete profile asks in a dialog whose Cancel has focus. The focus
commands move between Cancel and Delete, `dialog_confirm` presses the focused
one, and `dialog_cancel` cancels, as Cancel does. When
a global shortcut uses the profile, the dialog says why and offers only OK.
While the dialog is open, other Settings commands do nothing; closing the
window and app commands such as Quit still work.

New profile and Rename profile ask for a name in a dialog whose name field has
focus; Rename profile starts with the current name selected. The field takes
the `TextField` context, as other Settings fields do, so the text commands
edit it, `text_confirm` creates or renames, and `text_cancel` cancels. The
dialog focus commands move focus between the field, Cancel, and Create or
Rename, and on a button the other dialog commands apply. Create and
Rename stay disabled, and out of the focus order, while the name is blank,
unchanged, needs quoting, or names another profile.

The Licenses view takes the `Palette` and `TextField` contexts, so the
palette bindings apply there: `palette_select_next` and
`palette_select_previous` move the selection, `palette_page_down` and
`palette_page_up` scroll the license text, `palette_expand` and
`palette_previous_slot` move focus between the search and the buttons,
`palette_confirm` presses the focused button, the text commands edit the
search, and `palette_back` clears the search or returns to the About panel.

| Command | macOS | Linux |
| --- | --- | --- |
| `dialog_confirm` | `enter`, `space` | `enter`, `space` |
| `dialog_cancel` | `escape` | `escape` |
| `dialog_focus_next` | `tab`, `right` | `tab`, `right` |
| `dialog_focus_previous` | `shift-tab`, `left` | `shift-tab`, `left` |

### Menu

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
| `menu_close` | `escape`, `tab`, `shift-tab` | `escape`, `tab`, `shift-tab` |

### Notices

These bind only while a notice has focus (`notices`), which `focus_notices`
gives to the newest notice. Dismissing the last focused notice returns focus to
the terminal.

| Command | macOS | Linux |
| --- | --- | --- |
| `notice_next` | `down` | `down` |
| `notice_previous` | `up` | `up` |
| `notice_run_action` | `enter` | `enter` |
| `notice_dismiss` | `escape`, `delete` | `escape`, `delete` |

### Settings

These bind only in the Settings window. `settings_toggle` turns a switch on or
off; `settings_increase` and `settings_decrease` step a segmented control and
leave a switch alone. They step a Default, On, Off control in that order and
stop at the ends; stepping to Default removes the setting, as Reset does.

The sidebar's sections, the theme cards, the Light mode and Dark mode
cards, and the link modifier chips are roving groups: each is one tab stop,
and its items take the `SettingsGroup` context. The light and dark theme
cards are two groups, but the arrows move through both in the order they
show. On a group, `settings_next` (Down and Right) and `settings_previous`
(Up and Left) move between its items and wrap at the ends. They bind under
`SettingsGroup`, so the arrows keep stepping every other control, and since
that context sits inside the item's `SettingsControl`, they win over any
`SettingsControl` binding on the same key: binding an arrow to a stepping
command leaves the groups their arrows, and binding `settings_next` to
another key leaves the stepping alone.
Moving to a section shows it, moving to a theme card chooses that theme,
and moving to a mode card makes the theme cards choose that mode's theme.
`settings_toggle` turns the focused modifier chip on or off. Tab from the
sidebar moves into the shown section's first control, and Shift-Tab from that
control returns to the sidebar; the footer's links come after the section.
Moving focus with the keyboard scrolls the focused control into view; a click
does not. Selecting a section, by key, command, or click, scrolls the sidebar
to show it. Enter and Space reach a switch, segmented control, chip, theme card,
section, button, or key recorder only through these bindings, so removing a
binding removes its key there.

The window's text fields take the `TextField` context, so the text bindings
edit them: `text_confirm` saves a field, or from the search field moves to the
first match; `settings_focus_next` and `settings_focus_previous` move focus on
and back; `text_previous` and `text_next` step a number within its range;
`text_page_up` and `text_page_down` page a number by a tenth of its range, or
by ten steps when either end is open; and `text_cancel` puts a field's value
back or clears the search. A field's draft stays a draft while its context
menu is open; it saves when focus moves elsewhere, as it does on leaving the
field.
Home and End keep moving the caret in a number field. `settings_increase` and
`settings_decrease` step a Quake width or height slider by 5%, stopping at 5%
and 100%; on a slider, `settings_page_up` and `settings_page_down` page it by
10%, and `settings_first` and `settings_last` move it to 5% and 100%. Their
default keys carry `when = "control == slider"`, so Page Up, Page Down, Home,
and End stay unbound on other Settings controls. In a
shortcut's `when` field and the font family field, `text_next` and
`text_previous` move through the suggestions, opening them when they are
closed, `text_page_up` and `text_page_down` page through them, `text_confirm`
takes the highlighted one, and `text_cancel`, like `settings_focus_next` in a
`when` field, closes them before it acts on the field. The font field
highlights nothing until you move, so `text_confirm` saves the typed name.
Until you type in it, the font field's list shows every installed family, on
the one it holds. In an argument's number field, `text_next` and
`text_previous` step it. In Keybindings, `settings_search` focuses the
section's own search, where `text_confirm` moves to the first command, and
`settings_reset` on a command restores its defaults. The window
also reads `close_window`, `close_tab`, and `close_pane` as closing it, and
`next_tab` and `previous_tab` as moving between sections.

| Command | macOS | Linux |
| --- | --- | --- |
| `settings_search` | `cmd-f` | `ctrl-f` |
| `settings_focus_next` | `tab` | `tab` |
| `settings_focus_previous` | `shift-tab` | `shift-tab` |
| `settings_toggle` | `space`, `enter` | `space`, `enter` |
| `settings_increase` | `up`, `right` | `up`, `right` |
| `settings_decrease` | `down`, `left` | `down`, `left` |
| `settings_page_up` | `pageup` | `pageup` |
| `settings_page_down` | `pagedown` | `pagedown` |
| `settings_first` | `home` | `home` |
| `settings_last` | `end` | `end` |
| `settings_next` | `down`, `right` | `down`, `right` |
| `settings_previous` | `up`, `left` | `up`, `left` |
| `settings_reset` | `cmd-backspace` | `ctrl-backspace` |
