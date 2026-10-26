# Commands and keybindings

Every menu item, shortcut, and palette entry runs a command from one catalog.
Each command has a stable ID, a scope, and named typed arguments. Keybindings
live in the same file as the other settings; see
[Configuration](configuration.md) for its location and reload.

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
| `Palette` | The command palette or the About panel's Licenses view is open and has focus. |

## Commands

A command's scope says who runs it. Runtime commands run in the core against
the canonical session structure. The `tab`, `workspace`, and `session`
arguments cannot be written in the config for any command. The palette
prompts for them where a command needs one, and omitted targets default to the
invoking window's active tab, workspace, or session. Palette-scope commands
imply the `Palette` context, and dialog, menu, and notice commands imply
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
| `split_right` / `split_down` | Window | Splits the focused pane; the new pane opens beside or below it and takes focus. |
| `split_left` / `split_up` | Window | Splits the focused pane; the new pane opens before or above it and takes focus. |
| `close_pane` | Window | Closes the focused pane, asking first when it runs a job. Closing a tab's only pane closes the tab. |
| `focus_next_pane` / `focus_previous_pane` | Window | Focuses panes in layout order, wrapping. |
| `focus_pane_left` / `focus_pane_right` / `focus_pane_up` / `focus_pane_down` | Window | Focuses the adjacent pane in that direction; does nothing at the tab's edge. |
| `resize_pane_left` / `resize_pane_right` / `resize_pane_up` / `resize_pane_down` | Window | Optional `amount` in cells (1 to 100), default 5. Moves the focused pane's edge that way, keeping every pane at least 4 columns by 2 rows; does nothing without a neighbour on that side. |
| `toggle_pane_zoom` | Window | Shows the focused pane across its tab, or every pane again. Focus, split, and resize commands unzoom first. |
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

### Dialog

These bind only while a close confirmation or the About panel is showing
(`confirming`), so they take nothing from terminal input. The primary button
has focus when the dialog opens, and a repeated close shortcut never confirms.
In the About panel, the focus commands move between its Licenses, Copy Details,
and Close buttons, `dialog_confirm` presses the focused one, and
`dialog_cancel` closes it. Focus starts on Close.

The Licenses view takes the `Palette` context, so the palette bindings apply
there: `palette_select_next` and `palette_select_previous` move the selection,
`palette_page_down` and `palette_page_up` scroll the license text, the text
commands edit the search, and `palette_back` clears the search or returns to
the About panel.

| Command | macOS | Linux |
| --- | --- | --- |
| `dialog_confirm` | `enter` | `enter` |
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
| `menu_close` | `escape`, `tab` | `escape`, `tab` |

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
