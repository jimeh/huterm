# Pane input sync

Status: in progress.

## Outcome

A tab can turn on input sync. While it is on, keyboard input typed into the
tab's focused pane also reaches every other pane of the tab, as tmux's
`synchronize-panes` and iTerm2's "Broadcast Input to All Panes in Current
Tab" do. The user can see at a glance which tab syncs, and every pane it
reaches, with title bars shown or hidden.

## Decisions

### State lives in core, per tab

`Tab` gains `input_sync: bool`. `Mux::set_input_sync(workspace, tab, bool)`
validates the target like `set_pane_zoom`, emits
`HierarchyEvent::TabInputSyncChanged { tab, input_sync }` only on a change,
and leaves `Mux::revision` alone, because sync never changes which terminals
a close covers. `TabInfo` and `TabRecord` carry the flag, so a snapshot, a
tab a move opens, and an undo that reopens a removed tab all hold it, and
the projection digest covers it. The reducer applies the event and lists
the tab as touched; reconcile redraws any window holding a touched tab,
because a toggle changes no title.

Why core: a tab is core structure, and a flag there moves with the tab
between workspaces and windows, survives view replacement, and is visible to
a future text client through the same hierarchy stream. A client-side flag
would need carrying across every move and would be invisible to other
clients, as tmux's `synchronize-panes` is a window option every client sees.

### Fan-out lives in each client

Input reaches a terminal only through a viewer, whose queue keeps its order
and whose typing makes it the controlling viewer and returns the shared
viewport to live. So each client fans its own typing out through its own
viewers of the tab's other panes; core does not route one viewer's input to
other terminals, which would put Mux lookups on the lock-free input path.

The client sends the structured `TerminalInput`, never bytes. Each runtime
encodes it at dequeue against its own live modes (application cursor keys,
bracketed paste, and the rest), so every pane gets its own encoding.

### What syncs

- Synced: keys and characters `handle_keystroke` turns into input, Option
  composition results, text from the macOS native text path
  (`send_composed_text`: plain printable keys, IME commits, and replayed
  shortcut prefixes), and Paste, from the command, a binding, or a
  right-click action. tmux and iTerm2 both broadcast paste. The hook sits
  after translation, so each path syncs exactly what the focused pane gets.
- Per pane: mouse reports, wheel, focus reports, file drops (they target the
  pane under the pointer, as mouse input does), preedit, selection, scrolling,
  and every Huterm command except Paste. Bindings resolve before input, so
  a shortcut that splits or closes acts once.
- An exited focused pane takes no input and syncs nothing, as tmux skips a
  dead pane. An exited, revoked, or full target drops its copy through its
  own existing admission path; the other targets still receive theirs.

### Membership follows the tab

Sync reaches whatever panes the tab holds when the input is typed, from the
window's views of panes the projected layout places:

- A pane split off, or moved in, while sync is on receives input once its
  view is installed.
- A pane moved out stops receiving it. A pane moved to a new tab starts
  with sync off; nesting a tab into another tab's pane gives its panes the
  destination tab's state. Moving a tab's only pane elsewhere moves the
  tab itself, which keeps its flag.
- A pane a zoom hides receives nothing, as tmux skips panes that are not
  visible: nobody can see what it would type. Unzooming resumes it.
- A pane still installing, or still closing, receives nothing.
- A target scrolled back into history returns to live output, as the
  focused pane does when typed into, so its user sees what arrived.
- Copies arrive through the target view's own admission and never sync
  again; the source is the emitting view, not the window's focused pane.

### Command and bindings

`toggle_pane_input_sync`, Window scope, no arguments, acts on the active tab.
The window toggles on a background worker that reads and writes under one
Mux lock, so two quick toggles cancel out regardless of order.

Default bindings: `cmd-alt-i` on macOS, matching iTerm2, and
`ctrl-shift-alt-i` on Linux, following the Linux `ctrl-shift` mapping of
macOS `cmd`. Neither is bound today. Two modifiers plus a letter makes an
accidental toggle unlikely.

Surfaces: the command palette, the Settings keybindings "Panes" group, the
macOS Window menu beside the pane items, the pane title bar menu, and the
terminal menu's pane group while the tab is split or syncing. Context menu
rows read "Sync Pane Input" or, while on, "Stop Syncing Pane Input". The
macOS menu bar's titles are fixed, so its row keeps the catalog title,
"Toggle Pane Input Sync".

### Indication

All drawn without listeners, so GPUI gives them no hitbox, as the focus
accent and dim layer are.

- Each drawn live pane of a syncing tab gets a 2-point dashed inset frame
  in the theme accent, drawn over its terminal and title bar. This holds
  with title bars hidden, a single pane, or zoom. The solid focus cue draws
  over it, and its dashes keep that cue readable. An exited pane takes no
  input, so it gets no frame, and while the focused pane has exited no
  pane is framed, since typing reaches none (UX review).
- The tab item, and the title bar of each pane typing reaches, show the
  Lucide `radio` icon in the accent colour before the title. Fit tab
  widths count it.
- The palette row reads like the menu rows, "Sync Pane Input" or "Stop
  Syncing Pane Input", through the window's row titles, and its
  description says "broadcast", so that search finds it (UX review).
- Accent, never the error colour; nothing blinks.

No setting turns the indication off. The generated schema and
`docs/keybindings.md` gain the command and its default keys.

## Non-goals

- Syncing a subset of panes, or across tabs or windows.
- A tab context menu row, which would need a `tab` argument on the command.
- A toggle confirmation or a transient badge; revisit if the UX review asks.
- Persisting sync across restarts; restoration does not exist yet.

## Failure paths

| Case | Handling |
| --- | --- |
| Toggle on a missing, foreign, or closed tab | Mux error, reported as a "Pane input sync" notice |
| Target queue full | That pane's existing "Input rejected" warning; others unaffected |
| Target exited or revoked | Its admission drops the copy silently, as today |
| Source exited | No input, so nothing syncs |
| Pane moved out between key and fan-out | Fan-out reads current membership |

## Verification

| Concern | Evidence |
| --- | --- |
| Reducer applies and touches the tab; a missing tab resyncs | protocol hierarchy unit test |
| Setter emits once, no-op on repeat, rejects foreign targets, keeps revision | core Mux unit test, through `converge` |
| Moves: a new tab starts off; undo reopening a removed tab restores it | core moves and restore tests |
| Typing reaches both real shells in order, focused pane included | Tier 2 window test with two `ECHO` panes |
| Per-pane encoding | Tier 2 test: Up arrow gives `1b4f41` in a pane with DECCKM on and `1b5b41` in one without |
| Paste syncs with per-pane bracketing | Tier 2 test |
| Huterm commands act once | Tier 2 test: a split binding with sync on adds one pane |
| Sync off sends to the focused pane only | Tier 2 test |
| A pane split in while on receives input; a moved-out pane does not; an exited target does not break the others; a zoom-hidden pane does not | Tier 2 tests, the hidden pane read through the canonical snapshot |
| Toggle flips the projected flag and redraws the marks without another trigger | Tier 2 drawn-state test, plus menu unit tests |
| Frame and icons draw for a syncing tab, none otherwise | Tier 2 drawn-state test, plus the UX review |
| Default bindings, no conflicts | keymap unit test |
| Native typing, IME, Option composition, and `cmd-alt-i` on a real keyboard layout | UX review in the running app; Tier 2 cannot drive AppKit's text path |

File drops call `admit_input` directly and never reach the sync hook; that
is checked by inspection rather than a test, since no drop helper exists.

`mise run check`, focused tests, and `mise run verify` before handoff.
