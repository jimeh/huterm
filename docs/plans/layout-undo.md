# Layout undo

Status: implemented. Approved on 2026-10-27, with Jim's answers to its
seven open questions recorded under "Settled decisions". PR 1 (runtime
restore) merged in #279, and PR 2 (client history) closes the issue. Plans
[issue #256][issue-256] under [epic #12][issue-12], related to
[#29][issue-29] and [#30][issue-30]. This plan builds on the runtime hooks
that [the pane moves plan](pane-moves.md) delivered; all three of #29's PRs
(#257, #262, and #264) are on `main`. #30 lands after this work and records
its operations through it (see "How #30 plugs in").

A user who drops a multi-pane tab into another tab's pane by mistake gets a
nested layout. Pulling that subtree back out into its own tab by hand is
tedious. Layout undo reverses such a rearrangement with one key, and redo
reapplies it. Undo never creates or destroys a terminal or restarts a
process: it only puts existing panes back where they were.

## Current state

Observed on `main` at `d7bc7c0c`:

- Nothing records layout history. No default binding uses `cmd-z`,
  `cmd-shift-z`, `ctrl-shift-z`, or `ctrl-shift-y`.
- Every allocation advances `Mux::revision` before it checks for
  exhaustion (`crates/huterm-core/src/mux.rs:224-235`).
- `TabInfo::zoomed` must name a pane of a layout with more than one pane
  (`crates/huterm-protocol/src/hierarchy.rs:123`), and `set_pane_zoom`
  refuses a lone pane (`mux.rs:667`).
- `sync_hierarchy` (`crates/huterm-gpui/src/desktop/windows.rs:1399`)
  reconciles a drained batch before it releases completion waiters.
- `windows_to_visit` (`desktop/windows/projection.rs:453`) visits only
  windows that show a touched workspace or tab.

## What this builds on from #29

From the pane moves plan's first PR:

- Every move operation returns a `LayoutChange`: the record of each tab it
  touched, before and after, read under the Mux lock.
- `PanesRearranged`, the atomic multi-tab event, with its `Layout`,
  `Placed`, `Opened`, and `Removed` changes, ordered application, and the
  reducer's conservation check. A restore emits this event. Reopening a
  removed tab is an `Opened` change with a tab ID that is unknown at its
  step, which the reducer already accepts.
- The allocation preflight, which keeps a refused operation from advancing
  the revision.
- Viewer revocation for panes whose session changes, and the swap and move
  revision rules.

From the pane moves plan's second PR:

- Reconcile adoption keeps a pane's view when the pane changes tabs inside
  a window, so an undo keeps scroll and selection like any move.
- The move and swap commands, the first operations a window can record.
- The shared move completion that commands and drops both run. History
  records there, so drops are recorded with no further work once #29's
  drag PR lands, in either order.

This plan adds two things to those hooks: a `placed` list in
`LayoutChange`, and the `restore_tabs` operation. #29's drag PR landed
first, so drops are recorded from the client PR on.

## Scope

In scope:

- Per-window undo and redo of the last layout change: #29's swaps,
  moves, extractions, and tab drops (settled decision 1).
- One runtime restore operation guarded by the current state.
- `undo` and `redo` commands, bindings, and menu items.

Non-goals:

- #30's presets, cycling, and balancing. #30 records them as entries when
  it lands.
- Multi-level history. It can follow by growing only the client stack.
- Undoing splits, closes, renames, tab opens, focus, or zoom.
- Persisting history across restarts, or sharing it between windows.
- Keeping closed terminals alive so a close can be undone.

## Decisions

### Owner: the client window, with a runtime-guarded restore

Each window keeps its own history of the layout operations it initiated.
Undo sends the runtime one restore operation, which applies a recorded
snapshot only if the touched tabs still match what the operation left.
Alternatives considered:

- *Runtime history per session.* One window's undo would revert another
  window's change, which in a shared session surprises both users. A
  cross-session move belongs to two sessions' histories at once.
- *Runtime history per tab.* A move spans two tabs, and a tab dropped into
  a pane no longer exists to own its history.
- *Runtime history per attachment.* Workable, and it would survive a future
  client reconnecting over IPC. It adds runtime state and needs a way to
  publish availability for menus, for a benefit no client needs yet. The
  restore operation and the record types live in `huterm-protocol` and
  core, so a TUI client keeps its own stack with the same guarantees, and
  moving the stack into the runtime later changes no event.

A second window attached to the same session sees an undo as ordinary
structural events from another window. Its own history is untouched,
except that entries the change invalidated are pruned (see
"Invalidation").

### Snapshots, not inverse operations

An inverse "move it back beside X" fails whenever a flatten, a collapsed
split, or a removed tab changed the shape. Each history entry instead holds
the records of every tab the operation touched, before and after. Core
returns the records; the client wraps them with its own presentation data:

```text
LayoutChange { before: Vec<TabSnapshot>, after: Vec<TabSnapshot>,
               placed: Vec<TabId> }                                  core
TabSnapshot  { tab: TabId, record: Option<TabRecord> }       None: absent
TabRecord    { session, workspace, index, custom_name, layout, zoomed }

HistoryEntry { label, focus: PaneId, change: LayoutChange }        client
```

`LayoutChange`, `TabSnapshot`, and `TabRecord` come from #29. This plan adds
`placed`: the existing tabs whose workspace and position the operation set
on purpose, which are the tabs its event moved with a `Placed` change.
Core fills it while it builds the event. A tab whose index shifted only
because a neighbour was removed or opened is not in it.

`label` names the operation ("Move Pane", "Move Tab into Pane"). `focus` is
the pane the initiating window focused after the operation: the moved
pane, the swapped source pane, or the dropped tab's focused pane. A few
tab layouts per entry come to a few kilobytes at most.

### The restore operation

`Mux::restore_tabs(change: &LayoutChange, direction)` takes the recorded
change, including `placed`, and a direction of `Undo` or `Redo`. For
`Undo`, `from` is the change's `after` and `to` is its `before`. For
`Redo`, they are reversed. Under the lock, core checks the guard, applies
the restore, emits one `PanesRearranged`, and returns the change it made.

The guard compares each tab in `from` with the current state:

- An absent snapshot matches only an absent tab.
- A present snapshot matches a tab with the same shape, split IDs, pane
  IDs, and terminal IDs, in the same session. Shape means the tree of
  splits with its axes and child counts, and the panes in their places.
- A tab in `placed` must still be in its `from` workspace. Its position
  within that workspace is not compared.

The guard ignores every other field: weights, zoom, position, name, and the
workspace of a tab not in `placed`. A divider drag, a zoom, a rename, or a
reorder elsewhere therefore never blocks undo. A tab another client moved to
a different session does block it, because a window should not rearrange
tabs that now belong to another session's users.

The restore applies each tab in `to` field by field:

- **A tab that exists before and after the restore:** its layout returns
  to the `to` layout, weights included. Weights are the one deliberate
  exception to keeping later changes: when the shape differs, no later
  weights can carry over, so undo restores the proportions from before the
  operation. Its zoom becomes the `to` zoom only when the operation changed
  the tab's zoom and the current zoom still equals the `from` zoom.
  Otherwise the current zoom stays if its pane is in the restored layout,
  and clears if not. Either way, a restored layout of one pane has no zoom,
  as `set_pane_zoom` and the reducer require. Its workspace and index
  return to the `to` record only for a tab in `placed`. Its name always
  keeps its current value, so another client's rename, reorder, or zoom
  survives.
- **A tab the restore reopens:** it gets its recorded custom name and zoom,
  at its recorded index in its recorded workspace, clamped to that
  workspace's current length.
- **A tab the restore removes:** it is an extraction's new tab.

Core rejects a restore that would break any of these rules:

- Conservation holds exactly, as the reducer checks it.
- Every tab, split, and pane ID is at or below the allocation high-water
  mark (`next_id`).
- A reopened tab's ID is unused.
- No untouched tab uses a restored split ID.
- Every workspace the restore names exists, in the session its record
  names.

A refused restore changes nothing and emits nothing. A restore never spawns
or stops a runtime. It revokes the viewers of every terminal whose session
it changes, as a cross-session move does. It advances the revision when a
terminal changes tabs or a tab opens, is removed, or changes workspace or
position, all of which change what a close covers. It leaves the revision
alone when it only rearranges panes within their tabs, as a swap does.

### A removed source tab comes back as itself

Undoing a tab-into-pane drop, or a move that emptied its source, reopens
the tab with its original `TabId`. Reusing the ID is safe for three
reasons:

- Mux never allocates an ID twice.
- The restore accepts only IDs it has allocated (at or below `next_id`)
  that are not in use.
- The reducer's `TabOpened` and `Opened` already accept any unknown ID.

Reuse lets the window's tab MRU, the palette, and any external reference
converge on the same tab. A recorded workspace that no longer exists, or
that moved to another session, refuses the restore.

### What is recorded

| Operation | Recorded |
| --- | --- |
| Swaps, moves, extraction, tab into pane (#29) | Yes, one entry each |
| Presets, cycling, balancing (#30) | Yes, one entry each, once #30 lands |
| Divider drags and resize commands | No (settled decision 2) |
| Split, pane close, tab open or close | No |
| Zoom, focus | No: view state, not structure |

Splits and closes are excluded because undo would have to destroy a
terminal it created, or recreate one it closed. The first kills a process
the user may be using. The second is impossible without keeping closed
terminals alive, which Ghostty does for `undo-timeout` (5 s by default) but
which contradicts Huterm's rule that an explicit close terminates. Both stay
explicit. They still change shape, so the history entries they cut off are
pruned (see "Invalidation").

Divider resizes are excluded from history for two reasons. They reverse by
dragging. They would also flood the history and push out the structural
entries undo exists for. The guard ignores weights, so a resize never
blocks undo, and undo restores the touched tabs' weights from before the
operation, discarding later resizes of those tabs.

### Invalidation

Only the top of each stack is checked against the current state: the
newest undo entry's `after`, and the newest redo entry's `before`. Older
entries form a chain. Entry k's `after` is the state that undoing entry
k+1 recreates, so each entry becomes checkable exactly when it reaches the
top. A window's own consecutive operations on one tab therefore never
prune each other, and a swap of A and B followed by a swap of A and C
undoes in two steps.

Pruning runs after every applied batch, in a pass separate from window
reconcile. Reconcile visits only windows showing a touched workspace or
tab, but an entry can reference tabs in sessions the window does not show.
The pass visits every window whose stack tops reference a touched tab, and
repeats while a top fails its guard against the projection:

- The failing entry is dropped.
- Older entries that share a tab with it are dropped too, because their
  chain ran through the state that just changed.
- Older entries that share no tab with it stay, and each is checked in turn
  when it reaches the top.

A window's own operations need one exception. `sync_hierarchy` reconciles a
batch before it releases completion waiters. The event of a window's own
move, undo, or redo can therefore apply before that window's completion
pushes the new entry or transfers one between stacks, and a pruning pass in
between would compare the old top with the new state and drop it. A window
with a recorded operation in flight marks its history pending, and the
pass skips that window. The completion records the result, clears the
mark, and prunes the window against the projection as it is then. A failed
operation clears the mark the same way.

The cost is proportional to the stack tops, not the history depth. Menus
and availability read the pruned stacks.

A pane that exited but stayed open still matches. A pane closed by exit or
by hand changes the shape, which cuts the chain at that tab. If the
projection lags and the runtime refuses a restore, the window drops that
entry and the older entries that share its tabs, then shows "Cannot undo:
the layout changed". Undo never skips silently to an older entry. Each
restore re-checks its own guard under the lock, so pruning is about accurate
menus, not safety: no entry can apply against a state it was not recorded
for.

Undo and redo take their entry off its stack when they start, and hold it
in the in-flight operation until the completion pushes it onto the other
stack, or drops it after a refusal. Pruning therefore never sees an entry
that is being undone or redone, in either mode.

With multi-level history, the pending mark still covers undo and redo, not
only new operations. Between the start of an undo and the arrival of its
restore event, the next entry down expects the restored state, but the
projection still holds the old one. A batch in that gap, such as another
client renaming the tab, would otherwise prune that entry wrongly. The mark
holds from the start of a move, undo, or redo until its completion, as
described above.

Under settled decision 1, the first version keeps one entry per stack.
Chaining and dropping older entries then have nothing to act on. The
pending mark becomes unnecessary too: a pass that drops the old entry
while the window's own new operation is in flight drops an entry the
completion replaces anyway, and an undo or redo has already taken the only
entry off its stack. What remains is a guard check of one entry per
stack after each batch.

### Limit

One undo entry and one redo entry per window (settled decision 1). When
multi-level history ships, its depth is `LAYOUT_HISTORY_DEPTH`, a constant
of 50 rather than a setting, and the oldest entry is evicted first
(settled decision 5). Entries are small, and no terminal stays
alive for them, so no time limit is needed. A new recorded operation clears
the window's redo stack. Closing the window drops its history, and
retargeting its attachment to another session (#26) clears it. A window
emptied by its own move, such as a command moving its last pane into
another session, closes with its history. That is the accepted limit of
window-owned history (settled decision 6).

### After an undo or redo

The initiating window activates the tab holding the entry's `focus` pane, if
the window shows that tab, and focuses the pane.

### Commands, bindings, menus

| Command | Scope | macOS | Linux |
| --- | --- | --- | --- |
| `undo` | Window | `cmd-z` | `ctrl-shift-z` |
| `redo` | Window | `cmd-shift-z` | `ctrl-shift-y` |

Ghostty binds `undo` to `super+z` and `super+shift+t` and `redo` to
`super+shift+z` on macOS only, as "performable" bindings that pass the key
through when nothing can be undone ([Config.zig][ghostty-config] on
Ghostty's `main` at `ebd44999`, read on 2026-10-26). Its undo covers new and
closed windows, tabs, and splits, kept for `undo-timeout`, and is
unimplemented on Linux ([Binding.zig][ghostty-binding]). Huterm takes the
macOS keys and not `cmd-shift-t`, which reads as "reopen closed tab" and
would promise something this undo does not do. `cmd-z` has no terminal
meaning, and no Huterm default uses these keys. On Linux, `ctrl-z` is job
control and must reach the shell, so undo takes the shifted form, as copy
and paste do; `ctrl-shift-z` reaches applications only as plain `ctrl-z` in
legacy encoding, so binding it hides nothing a shell relies on.
`ctrl-shift-y` mirrors the `ctrl-y` redo convention. Neither is a current
default or a known desktop or IBus binding; `docs/keybindings.md` lists them
with the other Linux conflicts.

The commands are generic `undo` and `redo` rather than layout-specific
names, so a later undoable action joins the same history. They are
unavailable while the window is busy or a palette, dialog, or close
confirmation is open, so `cmd-z` in the palette's text field does not
rearrange panes. An open window menu does not block them: as for every
other command, a shortcut closes the menu and runs, and the menu's own Undo
and Redo items must stay usable while it is open. With an empty history
they do nothing, and the bindings still take the key. Ghostty instead
passes the key to the terminal. Huterm could do the same with a
conditional binding, but GPUI's native menu
matcher never sees a predicate that depends on window state, so the menu
would lose its shortcut. No terminal program reads `cmd-z` today. Once the
Kitty keyboard protocol lands (README future work), applications can, and
this choice should be revisited then (settled decision 7).

Menus: the macOS Edit menu gains Undo and Redo at its top; the Linux window
menu gains them above its Edit buttons. Titles are static ("Undo", "Redo");
the palette row shows the next entry's label, such as "Undo Move Pane".

## Races

| Race | Handling | Evidence |
| --- | --- | --- |
| A window's own move, undo, or redo event applies before its completion | Single-entry: undo and redo hold their entry off the stacks, and a new operation's completion replaces whatever a pass dropped. Multi-level: the window's history is pending, so pruning skips it until the completion records the result | Client unit test, event forced before completion for all three |
| Another client's batch arrives after an undo starts and before its restore event | Multi-level: the pending mark skips the window, so the next entry down is not checked against the pre-restore state. Single-entry: the stack is empty while the undo runs | Client unit test |
| An undo whose tabs changed since the projection last checked | The runtime refuses under its own guard. The entry and the older entries sharing its tabs are dropped, with one notice | Core, tier 2 |
| Another window renames, reorders, or zooms a tab before this window undoes a move in it | The guard ignores those fields, and the restore keeps their current values | Core |
| Another window resizes a split in a tab before an undo | The guard ignores weights; the undo applies and restores the weights from before the operation | Core |
| Another window moves a touched tab to another session | The guard refuses; the entry is dropped with one notice | Core, tier 2 |
| A pane in an entry is closed, by hand or on exit | The shape changes, so the chain is cut at that tab and its entries are pruned | Client unit test, tier 2 |
| A restore reopens a tab whose workspace was closed | Refused; nothing changes | Core |
| An undo moves a pane back into another session while a window there shows it | Viewers revoked as for any cross-session move; the source session's windows install it | Core, tier 2 |

## Testing and verification

Apply the global testing rules: each new test fails at its assertion before
its fix, and the runner output names it. Synchronize on events and shell
ACKs, never on sleeps.

- Core PTY tests with real shells: undo and redo of a swap, a move within a
  tab, a move across tabs, an extraction, a last-pane extraction to another
  workspace, a move into another session, and a tab dropped into a pane.
  Before and after each, every shell answers a unique ACK, `$$` is
  unchanged, `kill(pid, 0)` succeeds, the terminal and pane IDs are
  unchanged, and a line printed earlier is still in the snapshot.
- Protocol and core, for `restore_tabs`:
  - undo and redo of every #29 operation return the exact earlier state;
  - `placed` lists only tabs the operation's event placed;
  - a rename, a reorder, and a shared zoom by another client survive an
    undo;
  - a divider resize by another client does not block an undo, and the
    undo returns the weights from before the operation;
  - a pane moved beside a lone pane B, a shared zoom on B set by another
    client, then an undo: the restored single-pane tab has no zoom;
  - after a tab-into-pane drop removes a tab and another client then
    reorders the receiving tab, undo still applies, because the receiving
    tab's shifted index is not in `placed`;
  - a tab moved to another session by another client refuses the undo;
  - a missing workspace refuses;
  - a never-allocated tab, split, or pane ID refuses;
  - a reopened ID already in use refuses;
  - a restore that breaks conservation refuses;
  - every refusal emits nothing and leaves the revision unchanged.
- Reducer: a tab removed in one `PanesRearranged` and reopened with the
  same ID in a later one applies, and the differential test drives undo and
  redo of every #29 operation.
- Client history unit tests, for the single-entry first version:
  - a new operation replaces the undo entry and clears the redo entry;
  - pruning reaches a window that does not show the touched tab;
  - with the event forced before the completion, an undo lands its entry
    on the redo stack and a redo lands it back on the undo stack, and a
    new operation ends with only its own entry on top.
  Multi-level history adds the chain, depth eviction, and pending-mark
  tests listed in "Invalidation" when it ships.
- Tier 2:
  - undo of a tab-into-pane drop reopens the tab at its index, with its
    name and `TabId`, focuses the entry's pane, and keeps every view entity
    in the window;
  - a refused undo shows one notice;
  - `cmd-z` with the palette open edits nothing;
  - menu availability follows the stacks after another window invalidates
    the top entry.
- Native: the overlay smoke (`check-overlays.ts`, run by
  `smoke:macos-palette` and the Linux overlays step), where the other pane
  and tab drags run, drags a tab by its label into a pane, presses the
  platform undo key, and asserts the restored tab count, active tab, and
  layout in smoke state, and the same `$$` in each pane.
  `smoke:macos-menus` checks the Edit menu's Undo and Redo shortcuts.
  #29's drag PR landed first, so the client PR adds the tab-drop step
  directly.

Commands: `mise run check` while iterating; focused `cargo test -p
huterm-protocol`, `cargo test -p huterm-core mux::`, and `cargo test -p
huterm-gpui --features test-support`; `mise run test:ghostty-releasesafe`
for the PR with core PTY tests; `mise run verify` before each PR leaves
draft.

## Staging

Two PRs, stacked. Both prerequisites are on `main`, so the split no
longer waits on #29; it keeps the runtime contract reviewable on its own,
with its PTY and ReleaseSafe evidence, before the client builds on it.

1. **Runtime restore.** `placed` in `LayoutChange`, `restore_tabs`, its
   guard and refusals, and the protocol, reducer, core, and PTY tests.
2. **Client history.** The per-window entries, pruning, `undo` and `redo`
   with bindings and menus, recording in the shared move completion, and
   the client unit, tier 2, and native tests. It closes #256.

## How #30 plugs in

The presets, cycling, and balancing of #30 replace one tab's layout. Each
returns a `LayoutChange` with that tab before and after and an empty
`placed`, and records it through the same completion as a move. Undo then
runs `restore_tabs` with no runtime change. Balancing changes only weights,
which the guard ignores. Its entry therefore stays valid across later
resizes, and undo restores the weights from before the balance.

## Documentation

- `CONTEXT.md`: **Layout history** (a window's bounded record of the
  layout rearrangements it made, which undo and redo restore).
- `docs/agents/core-runtime.md`: the restore guard and its reuse of a
  removed tab's ID.
- `docs/agents/desktop-client.md`: per-window history, pruning, and the
  pending mark if multi-level history ships.
- `docs/keybindings.md`: `undo` and `redo`, and the Linux binding notes.
- This plan's `Status:` in each PR.

## Settled decisions

Jim accepted every recommendation on 2026-10-27.

1. **The first version undoes only the last layout change**: one undo
   entry and one redo entry per window. The record types and
   `restore_tabs` stay as planned, so multi-level history can come later by
   growing only the client stack. One entry removes the chain, the
   dropping of older entries, depth eviction, and the pending mark. The
   cost is that two quick mistakes are only half recoverable.
2. **Divider drags and resize commands are not recorded** (see "What is
   recorded").
3. **Linux keys are `ctrl-shift-z` and `ctrl-shift-y`**; macOS uses
   `cmd-z` and `cmd-shift-z`.
4. **A restored tab reuses the removed tab's `TabId`.**
5. **Multi-level history, when it ships, keeps 50 entries**, as a
   constant. It is out of scope for the first version.
6. **History is owned by each window and dies with it**, including a
   window that a move of its own emptied.
7. **`cmd-z` with nothing to undo always takes the key** for now. Revisit
   once Kitty keyboard protocol support exists.

[issue-12]: https://github.com/jimeh/huterm/issues/12
[issue-29]: https://github.com/jimeh/huterm/issues/29
[issue-30]: https://github.com/jimeh/huterm/issues/30
[issue-256]: https://github.com/jimeh/huterm/issues/256
[ghostty-config]: https://github.com/ghostty-org/ghostty/blob/ebd449995d321e90d6c73be3decb15558512b2ed/src/config/Config.zig
[ghostty-binding]: https://github.com/ghostty-org/ghostty/blob/ebd449995d321e90d6c73be3decb15558512b2ed/src/input/Binding.zig
