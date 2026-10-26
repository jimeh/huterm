# Pane moves

Status: proposed. Plans [issue #29][issue-29] within [epic #12][issue-12],
and proposes a separate layout-undo issue that #29's operations prepare
for. Builds on [the split panes plan](split-panes.md), which fixed the drop
zone geometry, the title bar as a drag handle, and settled decisions 1 to
3 for this issue.

A pane can swap places with a sibling, move beside another pane in the same
tab or in any other tab on the same runtime, or leave its tab for a new one.
A whole tab can be dropped onto a pane, where its layout nests. None of this
restarts a terminal or changes a pane's identity: a move changes which tab
holds a pane, never the pane or its terminal. Every path, keyboard, palette,
and drag, resolves to the same few Mux operations.

## Current state

Observed on `main` at `d7bc7c0c`, after #245, #246, #249, and #250:

- `PaneLayout` (`crates/huterm-protocol/src/pane_layout.rs`) validates on
  construction and offers `with_split`, `without_pane`, `with_weights`,
  `cycle`, `neighbor`, and `normalized_rects`. `with_split` inserts one
  `PaneInfo`; nothing inserts a subtree or exchanges two leaves.
- Mux (`crates/huterm-core/src/mux.rs`) has `split_pane`, `close_pane`,
  `resize_split`, `set_pane_zoom`, `move_tab`, and `move_workspace`.
  `move_tab` across sessions revokes the moved terminals' viewers through
  `invalidate_terminal_authority`. Every `allocate` advances
  `Mux::revision`.
- `HierarchyEvent` has `PaneLayoutChanged { tab, layout, zoomed }` for any
  change to one tab. The reducer's `layout_fits` rejects a layout whose
  pane or terminal another tab holds, so two sequential layout events that
  move a pane between tabs apply only in one order, and a drain that split
  them would show the pane in no tab.
- Reconcile (`desktop/windows/projection.rs`) drops pane views whose pane
  left its tab (`drop_left_panes`) and installs views for panes a tab gained
  (`missing_panes`). A pane that moves between two tabs one window shows
  would lose its view, viewer, scroll position, and selection, then get a
  new viewer.
- `desktop/windows/pane_geometry.rs` lays out frames, title bars, terminal
  bounds, and dividers. `drop_zone` does not exist yet; the split panes plan
  reserved it here.
- Tab reorder (`TabReorder`, `begin_reorder`, `update_reorder`,
  `finish_reorder` in `desktop/windows.rs`) runs on window-level capture
  listeners, keeps the preview inside the bar, sets `busy` while its commit
  runs, and cancels on Escape. Divider drags use the same listeners.
- The pane title bar (`tab_bar.rs::pane_title_bar`) focuses its pane on a
  left press and stops propagation; it starts no drag.
- `ArgumentKind` has `Tab`, `Workspace`, and `Session` identity kinds,
  which config bindings may not set, but no pane or side kind.
- No undo exists anywhere. No default binding uses `cmd-z`, `cmd-shift-z`,
  `ctrl-shift-z`, or `ctrl-shift-y`.

## Scope across #29, #30, #31, and the undo issue

| Issue | Owns |
| --- | --- |
| #29 | Swap and move commands (next, previous, four directions); moving a pane beside a pane in any tab on the runtime; extracting a pane into a new tab; moving a tab into a pane; the pane and tab drags inside one window, the drop overlay, and dropping a pane on the window's own tab bar; the atomic multi-tab hierarchy event; returning each change's before and after tab records |
| Undo (new issue) | Bounded per-window undo and redo of #29's operations and #30's presets and balancing, through one runtime restore operation |
| #30 | Presets, cycling, and balancing, each recorded as one undo entry |
| #31 | Every drag that crosses a window boundary: a tab or pane dropped on another window's tab bar or panes, and tear-out into a new window. It reuses #29's overlay, `drop_zone`, and Mux operations. |

Issue #29's drags end at their own window. A pane drag released outside
the window, or over another window, cancels and leaves every layout
unchanged. A tab drag released there keeps today's reorder behavior and
reorders within its own bar (see "Drags"). #31 later turns those releases
into transfers and tear-outs.

## Principles

The split panes plan's principles hold: one layout function, structure
shared and presentation local, and a single pane that looks and behaves as
a tab does. This plan adds:

- **Moves conserve terminals.** A rearrangement never creates or destroys a
  terminal, a pane, or a process. Core enforces it, and the reducer
  verifies it for every multi-tab event.
- **One event per operation.** A move that touches two tabs is one
  sequenced event, so no client ever observes a pane in no tab or two.
- **Views follow their panes.** A window that shows a pane before and after
  a move keeps the same view, viewer, scroll, and selection.
- **Undo restores snapshots, guarded by the runtime.** It never replays
  inverse commands.

## Decisions

### Layout transforms in `huterm-protocol`

Two transforms join the existing ones, so core, the reducer, and tests
share one implementation:

- **`with_inserted(target, side, node, split)`** inserts a subtree beside
  `target`. It generalizes `with_split`, which becomes a call with a pane
  node. Placement and weights follow split: the inserted node takes half
  the target's weight as a sibling when the target's parent splits on the
  side's axis, otherwise a new split identified by `split` replaces the
  target. When the inserted node is itself a split on the axis of the split
  that now holds it, it flattens into that split (settled decision 3) with
  the existing `normalize_child` scaling and `rebalance`, and a flatten that
  would pass `MAX_SPLIT_CHILDREN` keeps the nested split, as removal does.
  The inserted subtree keeps its internal proportions, its pane IDs, and
  the IDs of its nested splits. A flattened root split's ID disappears with
  it, as a flatten on removal does today. The flatten's floor of 1 per
  child and `rebalance` already keep a slot narrower than the subtree's
  child count valid, as they do for removal. The same refusals as
  `check_split` apply: unknown target, weight 1, full parent, and repeated
  identity.
- **`with_swapped(a, b)`** exchanges two leaves. Each slot keeps its weight;
  the `PaneInfo`s trade places. Swapping a pane with itself returns the
  layout unchanged.

Moves compose `without_pane` and `with_inserted`. `without_pane` already
returns the layout without the pane; a move reads the pane's `PaneInfo`
from the old layout first.

### Move semantics

Commands act on the window's focused pane, the source:

- **Swap next and previous** swap with `cycle(source, forward)`, wrapping.
- **Swap left, right, up, down** swap with `neighbor(source, side,
  history)`, the pane focus navigation would pick; with none, nothing
  happens.
- **Move left, right, up, down** remove the source and insert it on that
  side of the same neighbour: moving left from `[B | A]` gives `[A | B]`,
  and from `[B | (A over C)]` gives `[A | B | C]`. With no neighbour,
  nothing happens. Like a split, the inserted pane takes half the
  neighbour's weight, so a move resets the proportions of the split it
  lands in.
- **Move next and previous** pick the target before removal:
  `cycle(source, forward)` for next and `cycle(source, backward)` for
  previous. They then remove the source and insert it beside the target, on
  the side given by the axis of the target's parent split in the layout
  after removal. When the target is then the layout's only pane, the source's
  former parent axis is used. Next inserts on the trailing side (right or
  down) and previous on the leading side (left or up). A wrap reverses this:
  when the target is the first pane for next, or the last pane for previous,
  the source goes on the other side. So in `[A | B | C]`, moving C next gives
  `[C | A | B]`, and moving A previous gives `[B | C | A]`. In
  `[(A over B) | C]`, moving A next removes A, which collapses the rows
  split, and then inserts A to the right of B, giving `[B | A | C]`.
- **Move beside a pane** in any tab of the runtime, on a chosen side.
- **Move to a new tab** in any workspace of the runtime, by default right
  after the source tab in its workspace.
- **Move a tab into a pane** on a chosen side; its whole layout nests
  (settled decision 3).

Refusals and no-ops:

- A pane moved beside itself or swapped with itself is a no-op. So is a
  move whose result has the same shape as the current layout. Shape means
  the same tree of splits with the same axes and child counts, and the same
  panes in the same places, ignoring split IDs and weights. Dropping A on
  B's left edge in `[A | B]` is one example. Core computes the result with
  a placeholder split ID before allocating, so a no-op allocates nothing.
  It emits no event, leaves the revision alone, and returns an empty
  `LayoutChange`.
- A tab moved into one of its own panes is refused (`LayoutError::SelfTarget`,
  new).
- Extracting a tab's only pane into a new tab in the same workspace is a
  no-op. Into another workspace, it moves the tab and keeps its ID, name,
  and pane. It emits the same `PanesRearranged` event as other moves, with
  a `Placed` change (see "Hierarchy events"), so undo can move it back.
- Moving a tab's last pane beside a pane of another tab removes the emptied
  tab (the issue's rule).
- The initiating window preflights the result as a split does: a move or
  drop that would leave any pane of the destination tab under 4 by 2 cells
  in this window is refused with a notice, and the overlay shows that zone
  as refused.

### Core operations

```text
swap_panes(workspace, tab, a, b) -> Result<Committed<LayoutChange>>
move_pane(from: PaneRef, to: Destination) -> Result<Committed<LayoutChange>>
move_tab_to_pane(workspace, tab, to: Beside) -> Result<Committed<LayoutChange>>

PaneRef     { workspace, tab, pane }
Destination = Beside { workspace, tab, pane, side }
            | NewTab { workspace, before: Option<TabId> }
```

Each operation validates the scope and membership of every ID. It computes
every resulting layout with the transforms, using a placeholder for any
split ID it would allocate, and checks destination capacity and the no-op
rule. Then it checks that the allocator has room for every identity it
needs: one `TabId` for a new tab and one `SplitId` where a new split forms.
It does that with `next_id.checked_add(count)` before calling `allocate`,
because `allocate` advances the revision before it detects exhaustion
(`mux.rs:224-235`). Only then does it allocate, commit every touched tab,
emit one event, and return. Any refusal, including `IdExhausted`, changes
nothing, emits nothing, and leaves the revision alone. Other callers of
`allocate` keep their current behavior. No operation stops, spawns, or
joins a runtime, so none blocks on a terminal. They hold the Mux lock as
`resize_split` does.

Identity and processes:

- Pane IDs, terminal IDs, and `TerminalRuntime`s stay as they are. Only the
  `Tab` records holding them change. `Tab::terminals()` covers the new
  membership, so `subscribe_terminal`, close tickets, and close paths
  follow it with no change.
- A pane whose session changes, through a move into another session's tab
  or a new tab there, has its viewers revoked through
  `invalidate_terminal_authority`, as cross-session `move_tab` does today.
  Same-session moves revoke nothing: membership checks are per session.
- A move removes a tab only when it took that tab's last pane or the whole
  tab, and never runs `close_terminals`. The core guide's "moves retain
  empty parents" still holds for workspaces and sessions. A tab is not an
  empty parent: a tab with no panes cannot exist.
- `cleanup_spawn` (`windows.rs:540`) closes a pending tab whose window went
  away. Another client can move a pane into that tab before the cleanup
  runs, so the cleanup closes only the panes the spawn created. Any pane
  moved in survives.

Shared zoom: a swap keeps it, since pane IDs keep their terminals. A move
clears the source tab's zoom when the zoomed pane left or one pane remains,
and clears the destination tab's zoom, matching split. A new tab starts
unzoomed.

`Mux::revision`: swaps leave it alone, because they change no close
ticket's scope, matching `resize_split`. Every move that commits advances
it. Cross-tab moves change a tab close's scope, and within-tab moves may
allocate a split anyway. A pending close assessment covering a moved
terminal therefore goes stale and reassesses with the new scope.

Cross-runtime targets: every ID is scope-checked, so a destination from
another Mux fails with `ForeignRuntime` before any change. Matching socket
names grant nothing.

Every operation returns a `LayoutChange` (see "Undo and redo"): each
touched tab's record before and after, read under the lock. It is empty for
a no-op. #29 uses it in tests, and the undo issue records it.

### Hierarchy events

Changes inside one tab (swaps, within-tab moves, and later #30's presets)
emit the existing `PaneLayoutChanged`. Changes that span tabs emit one new
event:

```text
PanesRearranged { changes: Vec<TabChange> }

TabChange = Layout { tab, layout, zoomed }
          | Placed { tab, workspace, index }
          | Opened { workspace, tab: TabInfo, index }
          | Removed { tab }
```

The split panes plan reserved a `PaneMoved` event. A single list of tab
changes covers every case:

- a pane move: two `Layout` changes;
- an extraction: `Layout` and `Opened`;
- emptying the source: `Removed`;
- a lone pane extracted to another workspace: `Placed`, which is
  `TabMoved` inside the atomic event;
- a tab dropped into a pane: `Removed` and `Layout`;
- the undo issue's restores, which may reopen a removed tab or place a tab
  back in its workspace.

Changes apply in list order, and each `index` is a final position at that
step, as `TabMoved` and `TabOpened` define it today. A tab may appear in at
most one `Layout` and one `Placed`, and never alongside an `Opened` or a
`Removed`. Core emits `Removed`, then `Placed`, then `Opened`, then
`Layout`, so indexes stay unambiguous.

The reducer applies the event all or nothing, under the existing envelope
rules: a stale or duplicate sequence is `Stale`, and a gap or foreign stream
is `ResyncRequired`. It applies the steps to a scratch copy of the touched
workspaces' tab orders and the touched tabs' records, then checks the
result:

- each `Layout`, `Placed`, and `Removed` names a known tab;
- each `Opened` names a tab unknown at its step, in a known workspace;
- every index is valid at its step;
- each new layout is valid, and its zoom is one of its panes;
- **conservation:** the multiset of `PaneInfo` values (pane ID, terminal
  ID, and fallback name) across the touched tabs is identical before and
  after the event. No untouched tab holds any of them, and every new
  layout's split IDs belong to the stream. A pane therefore cannot change
  terminals, and no terminal can appear or vanish.

Only then does the reducer commit the scratch state and indexes. A violation
leaves the state untouched and is `Inconsistent`. `Removed` differs from
`TabClosed` only in meaning: its panes now live in another tab of the same
event. `Touched` gains every touched tab in `tabs` and `layouts`, and their
workspaces, old and new, in `workspaces`.

The digest describes state, not events, so it does not change. #44
serializes the new event with the others.

### Client reconciliation

Reconcile gains two steps before membership removal.

**Adoption** finds each pane view the window holds whose pane the
projection now holds in a different tab of the window's workspace. The
projection must be at or past the view's committed sequence. Reconcile
moves that `PaneView` into the destination tab's `TabView`. The move keeps
the view entity, viewer, scroll, selection, and exit state, and restarts
only the pane's activity drain, which routes refreshes by tab.

If the window has no view of a projected tab whose panes it already shows
elsewhere, adoption builds that tab's `TabView` from the adopted views. It
does so synchronously, with no worker, subscription, or Mux lock. Only
panes the window does not show yet go through the existing install path.
An install already in flight for that tab still completes, and as today it
pushes only panes the window still has no view of. At worst it subscribes
one extra viewer, which it then drops.

Removal then finds nothing to drop for the moved panes. An emptied source
`TabView` is dropped as a removed tab is. Each adopted tab republishes its
`TabEntry` pane titles and tab label through `set_pane_titles` and
`set_tab_title`.

**Pane order** sorts every touched tab's retained `PaneView`s into the
projection's traversal order, with `in_layout_order`, whenever they differ.
Today that sort runs only on install (`view_install.rs:341`). After a swap
or a move within a tab, the views would otherwise keep their old order, and
the focus fallback in `remove_panes` (`windows.rs:2833`), which takes the
first retained view, would pick the wrong pane. Sorting reorders
`PaneView`s and recreates nothing.

Adoption needs the same session: a pane that left the window's session lost
its viewer, so its view is dropped and windows of the destination session
install it.

What each window does after a move:

| Window | Focus | Active tab | Zoom |
| --- | --- | --- | --- |
| The initiating window | The moved pane, or the swapped source pane | The destination tab, when the window shows it | Cleared in the source and destination tabs |
| Another window showing the source or destination tab | Unchanged unless its focused pane left that tab; then the existing fallback | Unchanged unless its active tab was removed; then the tab that received its focused pane, else the existing removal rule | Kept while its pane stays in that tab |

A moved pane joins the end of the destination tab's focus history in other
windows and leaves the source tab's. In the initiating window it becomes
the most recent.

Selection and scroll stay on the view. A move that changes the pane's size
resizes its terminal through the normal geometry path, so the existing
rules apply: a resize that advances the content generation clears a
selection, as any resize does, and the shared viewport stays anchored by
the runtime. That is the issue's "preserve relevant initiating-view
scroll/selection where valid".

Completions follow the committed-sequence rule. A move commit sets `busy`
like a reorder, waits for its sequence, reconciles, then applies the focus
and activation above.

Pane closes follow the pane, not the tab it was in when the close was
taken. Owed exit-closes are already keyed by pane, and `holds` checks that a
window shows the pane. Two paths still capture the tab and would lose the
close:

- `current_close_target` (`windows.rs:8642`) drops a
  `CloseTarget::Pane { tab, pane }` whose pane now sits in another tab.
- An assessment that fails with `UnknownPane` lands in
  `close_assessment_failed` (`windows.rs:8504`). That path shows "Cannot
  assess close" and settles an exit-close.

PR 2 changes both:

- A Pane target re-resolves its tab from the projection each time it is
  requested or retried. It stays current while any tab in the window shows
  the pane.
- An `UnknownPane` or `UnknownTab` failure is treated as stale when the
  refreshed projection still holds the pane in a tab the window shows. The
  close reassesses with the new tab and shows no notice. `UnknownTab` is
  the result when the move took the source tab's last pane, because
  `prepare_close` validates the tab before the pane
  (`lifecycle.rs:339`, `mux.rs:687`).
- A pane that left the window's session returns to owed, which is the
  existing rule for a close that never reached Mux.

### Commands, arguments, and pickers

Two argument kinds join the catalog:

- `ArgumentKind::Pane`, an identity like `Tab`: the palette prompts with a
  picker, config bindings cannot set it, and the schema omits it. Rows list
  every pane in the hierarchy as "workspace › tab › pane title", most
  recently focused first, as tab rows do. The window's own focused pane is
  excluded for move commands.
- `ArgumentKind::Side`: left, right, up, or down, carried as
  `CommandValue::Text`, validated against those four names, emitted as a
  schema enum, settable in bindings, and prompted in the palette with a
  four-row picker.

| Command | Scope | Arguments |
| --- | --- | --- |
| `swap_pane_next`, `swap_pane_previous` | Window | none |
| `swap_pane_left`, `_right`, `_up`, `_down` | Window | none |
| `move_pane_next`, `move_pane_previous` | Window | none |
| `move_pane_left`, `_right`, `_up`, `_down` | Window | none |
| `move_pane_to_pane` | Window | `pane` (Pane, prompted), `side` (Side, prompted) |
| `move_pane_to_new_tab` | Window | `workspace` (Workspace, optional, prompted; default this window's) |
| `move_tab_to_pane` | Window | `tab` (Tab, optional; default active), `pane` (Pane, prompted), `side` (Side, prompted) |

Separate commands per direction keep the #28 pattern. `move_tab_to_pane`
gives the tab drop a keyboard and palette equivalent, so the acceptance
criterion "palette pickers and explicit command arguments produce
equivalent layouts" covers drops as well. Workspace pickers list every
session's workspaces, which is how a pane reaches another session.

Availability follows #28: unavailable while the window is busy or shows a
close confirmation; directional and next or previous commands also need a
split tab. A command whose target vanished between picking and running
fails with `StaleTarget`, as other identity commands do.

No default bindings in #29 (open question 2). Menus:

- Terminal context menu: Move Pane to New Tab, shown in split tabs.
- macOS Window menu and the Linux window menu: Move Pane to New Tab.
- Tab menu: Move Tab into Pane…, which opens the palette on that command
  with `tab` filled.
- Everything else is in the palette.

### Drags

Two new drags join the window's capture listeners, beside reorder and
divider drags, and like them own motion and release once started, stop
propagation, keep terminal focus so no application sees a focus report, and
never reach a terminal.

**Pane drag.** A left press on a title bar's title area, not its buttons,
focuses the pane as today; motion past `TAB_DRAG_THRESHOLD` starts a
`PaneDrag { tab, pane, origin, pointer, target }`. Title bars exist only in
split tabs (settled decision 1), so a lone pane is dragged from the tab bar
as its tab.

**Tab drag onto panes.** Today a reorder projects every pointer position
onto the bar's axis and clamps it, including a release outside the window
(`desktop-input.md`, "Tab reordering"; `windows/tests.rs:574`). A sloppy
reorder that dips below a top bar therefore still reorders. #29 keeps that
rule and adds one deliberate exception, so that a reorder cannot become a
mis-drop.

A reorder enters pane mode only when the pointer is over a pane frame of
the active tab other than the dragged tab's own panes, and at least
`PANE_DROP_DEPTH` (32 points) past the tab bar's inner edge, measured
perpendicular to the bar. It leaves pane mode when either condition stops
holding.

In pane mode, the bar preview returns to the tab's slot, dimmed. A release
over an edge zone nests the tab, and a release over a centre zone does
nothing. Outside pane mode, every position reorders exactly as today,
including a release outside the window. The content area shows the active
tab, so dragging the active tab over its own panes never enters pane mode.
The desktop input guide records the exception with PR 3.

**Targets.** Each motion resolves a target from the last drawn frame:

- For a pane drag over a pane frame other than the source pane:
  `drop_zone(frame, point)`, all five zones.
- For a tab drag in pane mode: the four edge zones. Its centre offers
  nothing (settled decision 2).
- For a pane drag over the tab bar: an insertion slot from `TabStrip::slot`,
  which extracts the pane into a new tab there. Overflow autoscroll works
  as it does for reorder.
- For a tab drag outside pane mode: today's reorder slot.
- For a pane drag anywhere else, including outside the window: no target.

**Overlay.** Drawn above the panes and below menus and dialogs:

- an edge zone tints the half of the target frame the dropped pane or
  layout would occupy;
- the centre zone outlines the whole target frame and the source pane, the
  two that swap;
- a tab bar target draws the reorder insertion marker;
- a zone the minimum size refuses is drawn in the refused style, and a
  release there does nothing;
- a small chip with the dragged pane's or tab's title follows the pointer.

Colours come from `TabColors`: the accent at reduced opacity for tints and
outlines, the error colour for refused zones.

**Release.** A release with a target runs the matching Mux operation on a
worker with `busy` set, as reorder does: centre is `swap_panes`, an edge is
`move_pane` or `move_tab_to_pane` with `Beside`, a tab bar slot is
`move_pane` with `NewTab`. Each of these cancels the drag and changes no
pane layout:

- a pane-drag release without a target;
- a tab-drag release over a centre zone or a refused zone in pane mode;
- Escape;
- window blur;
- a tab switch;
- a close request;
- an overlay opening;
- a layout change that removes the source pane or tab.

A tab drag released outside pane mode reorders, as today. A refused
operation, such as a target that vanished between the frame and the
commit, shows one notice. A drop is an explicit request, unlike a divider
drag's silent snap-back.

### `drop_zone`

`drop_zone(frame, point) -> Option<Zone>`, with `Zone = Edge(SplitDirection)
| Centre`, lives in `desktop/windows/pane_geometry.rs` as the split panes
plan fixed it: the outer quarter of the frame, title bar included, on each
side is that edge's zone; where two edge zones overlap near a corner, the
edge nearer the pointer as a fraction of the frame's width or height wins,
ties going to the top or bottom edge; the rest is the centre. A point
outside the frame has no zone.

### Undo and redo

The undo work ships as its own issue (see "Staging"), but #29's operations
are shaped for it, so the design is settled here.

**Owner: the client window, with a runtime-guarded restore.** Each window
keeps its own history of the layout operations it initiated. Undo sends the
runtime one restore operation that applies a recorded snapshot only if the
touched tabs still match what the operation left. Alternatives considered:

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

**Snapshots, not inverse operations.** An inverse "move it back beside X"
fails whenever a flatten, a collapsed split, or a removed tab changed the
shape. Each history entry instead holds the records of every tab the
operation touched, before and after. Core returns the records; the client
wraps them with its own presentation data:

```text
LayoutChange { before: Vec<TabSnapshot>, after: Vec<TabSnapshot>,
               placed: Vec<TabId> }                                  core
TabSnapshot  { tab: TabId, record: Option<TabRecord> }       None: absent
TabRecord    { session, workspace, index, custom_name, layout, zoomed }

HistoryEntry { label, focus: PaneId, change: LayoutChange }        client
```

`placed` lists the existing tabs whose workspace and position the
operation set on purpose, the tabs its event moved with a `Placed` change.
A tab whose index shifted only because a neighbour was removed or opened is
not in it.

`label` names the operation ("Move Pane", "Move Tab into Pane"). `focus` is
the pane the initiating window focused after the operation: the moved
pane, the swapped source pane, or the dropped tab's focused pane. A few
tab layouts per entry come to a few kilobytes at most.

**The restore operation.** `Mux::restore_tabs(change: &LayoutChange,
direction)` takes the recorded change, including `placed`, and a direction
of `Undo` or `Redo`. For `Undo`, `from` is the change's `after` and `to` is
its `before`. For `Redo`, they are reversed. Under the lock, core checks the
guard, applies the restore, emits one `PanesRearranged`, and returns the
change it made.

The guard compares each tab in `from` with the current state:

- An absent snapshot matches only an absent tab.
- A present snapshot matches a tab with the same shape, split IDs, pane
  IDs, and terminal IDs, in the same session.
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
  as `set_pane_zoom` and the reducer require (`mux.rs:667`,
  `hierarchy.rs:123`). Its workspace and index return to the `to` record only
  for a tab in `placed`. Its name always keeps its current value, so
  another client's rename, reorder, or zoom survives.
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

A refused restore changes nothing and emits nothing. The restore never
spawns or stops a runtime, revokes viewers of terminals whose session
changes, and advances the revision.

**A removed source tab comes back as itself.** Undoing a tab-into-pane
drop, or a move that emptied its source, reopens the tab with its original
`TabId`. Reusing the ID is safe for three reasons:

- Mux never allocates an ID twice.
- The restore accepts only IDs it has allocated (at or below `next_id`)
  that are not in use.
- The reducer's `TabOpened` and `Opened` already accept any unknown ID.

Reuse lets the window's tab MRU, the palette, and any external reference
converge on the same tab. A recorded workspace that no longer exists, or
that moved to another session, refuses the restore.

**What is recorded.**

| Operation | Recorded |
| --- | --- |
| Swaps, moves, extraction, tab into pane (#29) | Yes, one entry each |
| Presets, cycling, balancing (#30) | Yes, one entry each |
| Divider drags and resize commands | No (open question 4) |
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

**Invalidation.** Only the top of each stack is checked against the
current state: the newest undo entry's `after`, and the newest redo entry's
`before`. Older entries form a chain. Entry k's `after` is the state that
undoing entry k+1 recreates, so each entry becomes checkable exactly when
it reaches the top. A window's own consecutive operations on one tab
therefore never prune each other, and a swap of A and B followed by a swap
of A and C undoes in two steps.

Pruning runs after every applied batch, in a pass separate from window
reconcile. Reconcile visits only windows showing a touched workspace or
tab (`windows_to_visit`, `projection.rs:453`), but an entry can reference
tabs in sessions the window does not show. The pass visits every window
whose stack tops reference a touched tab, and repeats while a top fails its
guard against the projection:

- The failing entry is dropped.
- Older entries that share a tab with it are dropped too, because their
  chain ran through the state that just changed.
- Older entries that share no tab with it stay, and each is checked in turn
  when it reaches the top.

A window's own operations need one exception. `sync_hierarchy`
(`windows.rs:1399`) reconciles a batch before it releases completion
waiters. The event of a window's own move, undo, or redo can therefore
apply before that window's completion pushes the new entry or transfers one
between stacks, and a pruning pass in between would compare the old top
with the new state and drop it. A window with a recorded operation in
flight marks its history pending, and the pass skips that window. The
completion records the result, clears the mark, and prunes the window
against the projection as it is then. A failed operation clears the mark
the same way.

The cost is proportional to the stack tops, not the history depth.
Menus and availability read the pruned stacks.

A pane that exited but stayed open still matches. A pane closed by exit or
by hand changes the shape, which cuts the chain at that tab. If the
projection lags and the runtime refuses a restore, the window drops that
entry and the older entries that share its tabs, then shows "Cannot undo:
the layout changed". Undo never skips silently to an older entry. Each
restore re-checks its own guard under the lock, so pruning is about accurate
menus, not safety: no entry can apply against a state it was not recorded
for.

**Limit.** 50 entries per window, a constant (`LAYOUT_HISTORY_DEPTH`), not
a setting. The oldest entry is evicted first. Entries are small, and no
terminal stays alive for them, so no time limit is needed. A new recorded
operation clears the window's redo stack. Closing the window drops its
history, and retargeting its attachment to another session (#26) clears
it. A window emptied by its own move, such as a command moving its last
pane into another session, closes with its history. That is the accepted
limit of window-owned history; open question 7 offers attachment-owned
history as the alternative.

**After an undo or redo**, the initiating window activates the tab holding
the entry's `focus` pane, if the window shows that tab, and focuses the
pane.

**Commands, bindings, menus.**

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
unavailable while the window is busy or a palette, menu, dialog, or close
confirmation holds focus, so `cmd-z` in the palette's text field does not
rearrange panes. With an empty history they do nothing, and the bindings
still take the key. Ghostty instead passes the key to the terminal. Huterm
could do the same with a conditional binding, but GPUI's native menu
matcher never sees a predicate that depends on window state, so the menu
would lose its shortcut. No terminal program reads `cmd-z` today. Once the
Kitty keyboard protocol lands (README future work), applications can, and
this choice should be revisited then (open question 8).

Menus: the macOS Edit menu gains Undo and Redo at its top; the Linux window
menu gains them above its Edit buttons. Titles are static ("Undo", "Redo");
the palette row shows the next entry's label, such as "Undo Move Pane".

## Races

| Race | Handling | Evidence |
| --- | --- | --- |
| Another window changes the dragged pane's tab during a drag | Targets resolve from the last drawn frame each motion; a vanished source pane or tab cancels the drag; a vanished target leaves the pointer over no zone | Tier 2 |
| The dragged pane exits mid-drag | With `close_on_exit = false` the pane stays and the drag continues; an exit-close that removes it cancels the drag | Tier 2 |
| The target pane closes between the last frame and the commit | The operation fails with `UnknownPane` before mutating; one notice | Core, tier 2 |
| Another window moves the same pane first | The second commit finds the pane in another tab and fails with `UnknownPane` against its stale `PaneRef`; one notice | Core |
| A move commits between a tab close's assessment and commit | The revision moved: `StaleClose`, reassess with the new scope | Core lifecycle test |
| A swap commits while another window's close confirmation shows | Swaps leave the revision alone; the confirmation stays valid | Core lifecycle test |
| A tab drop removes a tab another window is confirming a close for | The commit is stale. The existing rule dismisses a tab confirmation whose tabs all left the window, so the panes now in the destination tab are not closed | Tier 2 |
| A move lands between a pane close's take and its assessment | The Pane target re-resolves its tab from the projection. An `UnknownPane`, or an `UnknownTab` when the move emptied the source tab, reassesses without a notice while the pane is still shown | Tier 2, user close and exit-close, with the source tab keeping other panes and with the move taking its last pane |
| A move lands between a pane close's stale commit and its retry | `current_close_target` keeps the target while any tab in the window shows the pane | Tier 2 |
| A pane is moved into a pending spawn's tab whose window went away | `cleanup_spawn` closes only the spawned panes; the moved pane survives | Core PTY test |
| A divider drag in another window when its split changes | Existing rule: cancelled when the split vanishes or its children change | Existing tier 2 |
| A move empties another window's last tab | That window takes an emptied close, which refuses while its workspace still holds tabs | Tier 2 |
| A cross-session move while the source window shows the pane | Viewers revoked; the source window drops the view; the destination session's windows install it | Core, tier 2 |
| A window's own move, undo, or redo event applies before its completion | The window's history is pending, so pruning skips it until the completion records the result | Tier 2, event forced before completion for all three |
| An undo whose tabs changed since the projection last checked | The runtime refuses under its own guard. The entry and the older entries sharing its tabs are dropped, with one notice | Core, tier 2 |
| Another window renames or reorders a tab before this window undoes a move in it | The guard ignores those fields, and the restore keeps their current values | Core |
| A pane's terminal events interleave with its move | Terminal events are routed by pane; the adopted view keeps receiving them | Tier 2 |
| A drain sees the move and a later layout change in one batch | Reconcile is level-triggered and adopts from final state | Reconcile unit test |

## Experience

- Drags start from the pane title bar and the tab bar, where the thing
  dragged already is. Every drop has a palette and menu equivalent, so
  keyboard users get the same operations.
- The overlay previews the result region, not just the target, so an edge
  drop shows which half the pane will take. Refused zones look refused.
- Errors are notices with the operation's name; a cancelled drag says
  nothing.
- A mis-drop is undone with one key once the undo issue lands, which is why
  it lands before the drags (see "Staging").
- The tab bar target uses `TabStrip`, which already handles all four
  positions, both styles, and overflow. Tier 2 covers every position, and
  the native smokes cover the top bar. Panes as small as the minimum keep
  their four edge zones by fraction.

## Testing and verification

Mapped to #29's acceptance criteria, then the undo issue's. Apply the
global testing rules:

- Each new test fails at its assertion before its fix, or under one targeted
  perturbation for a test over existing behavior.
- The runner output names each new test.
- A "no byte reached the shell" assertion is ordered: after the gesture,
  the smoke or test sends a unique probe line and requires that the shell's
  next output is that probe's ACK, with nothing before it.

Layout equality across two runs, such as palette against explicit
arguments, compares a canonical form. Each pane is replaced by its index in
the fixture's original traversal order, split IDs are erased, and axes,
child order, and weights stay. Fresh split IDs therefore cannot make equal
layouts differ, and a mis-ordered result cannot compare equal.

### Survival of PTYs, IDs, history, and siblings (criterion 1)

- Core PTY tests with real shells, one per path:
  - a swap;
  - a move within a tab;
  - a move across tabs;
  - an extraction;
  - a last-pane extraction to another workspace;
  - a move into another session;
  - a tab dropped into a pane;
  - a pane moved into a pending spawn's tab before `cleanup_spawn` runs;
  - in the undo issue, undo and redo of each of these.

  Before and after each one:
  - every shell answers a unique ACK;
  - `$$` is unchanged and `kill(pid, 0)` succeeds;
  - the terminal and pane IDs are unchanged;
  - a line printed before the move is still in the snapshot;
  - sibling shells still answer.

  Synchronize on ACK output, never on sleeps.
- Core: the differential reducer test drives every new operation and
  checks the reducer's state against a fresh snapshot after each. It cannot
  catch a placement rule both sides get wrong, so the protocol tests below
  assert expected layouts directly.
- Tier 2: a pane moved between two tabs the window shows keeps its
  `TerminalView` entity, viewer, scroll offset, and selection, and no new
  viewer subscribes. The fixture gives the source and destination slots the
  same size, so the grid does not change and the selection must survive.
  A second case with a different size checks that the selection clears.
- Tier 1 (`view_tests.rs`): the view keeps a selection across a geometry
  push that leaves its grid unchanged, and clears it after a resize that
  advances the content generation. This is the "where valid" boundary.
- Tier 2: after a swap and after a move within a tab, the retained
  `PaneView`s are in projected traversal order, with the same entities. The
  test then closes the focused pane and checks that focus falls back to the
  first pane in the new order.

### Failure and edge paths (criterion 2)

- Protocol: `with_inserted` on each side, with the parent on the axis and
  across it. Cases:
  - a nested subtree kept intact, with its nested split IDs;
  - a same-axis subtree flattened, with weight scaling, its root split ID
    gone, and a slot narrower than its child count;
  - a flatten skipped at the child limit;
  - each refusal.

  Also: `with_swapped` with equal and different weights, and with itself;
  each move rule's expected layout, including every example in "Move
  semantics", both wraps, a target left alone at the root, directional
  moves and swaps stopping at each edge, and the shape-equality no-op.
- Reducer, for `PanesRearranged`:
  - it applies with the expected `Touched`;
  - a `Placed` change with a last-pane extraction applies;
  - each of these is `Inconsistent` and leaves the state untouched: an
    unknown tab, a tab in two incompatible changes, an `Opened` with a known
    ID, an invalid index at its step, a layout reusing an untouched tab's
    pane, a pane paired with a different terminal, and a conservation
    violation;
  - stale and gap rules hold;
  - a tab removed and later reopened with the same ID applies.
- Core:
  - last-pane moves remove the source tab;
  - same-pane moves, swaps with itself, and shape-equal moves emit nothing,
    allocate nothing, and keep the revision;
  - a tab moved into its own pane is refused;
  - a stale `PaneRef` fails with `UnknownPane`;
  - a destination from a second Mux fails with `ForeignRuntime`;
  - failed destination creation emits nothing, keeps the revision, and
    keeps both tabs. It is forced with no identities left, for each path
    that allocates: an extraction (one `TabId`), and a move or tab drop
    whose target's parent splits across the side's axis (one `SplitId`).
    No path allocates more than one identity;
  - cross-session moves revoke viewers, and same-session moves do not.
- Core lifecycle: a move between assessment and commit makes the commit
  stale, and the retry covers the new scope; a swap between them does not.
- Tier 2, concurrent view changes, each with an explicit order of commit
  and event:
  - a second window focusing, zooming, or dragging a divider in the
    destination tab while a move commits;
  - a focused pane moved away by another window;
  - an active tab dropped into a pane by another window, so the window
    activates the receiving tab;
  - a pane close and an exit-close taken before a move and assessed or
    retried after it.

### Drop zones and gestures (criterion 3)

- `pane_geometry` unit tests: `drop_zone` for each edge, the centre, corner
  overlaps on both diagonals, exact ties going to top or bottom, a title
  bar point, and points outside the frame.
- Tier 2 drag tests through `Window::dispatch_event`:
  - a pane dropped on each of a sibling's five zones;
  - a pane dropped on the tab bar at a slot, for all four tab bar
    positions;
  - a tab dropped on each edge of a pane, and on a centre, which does
    nothing;
  - a multi-pane tab dropped onto a pane, nesting its layout with its pane
    and nested split IDs;
  - a same-axis tab join flattening;
  - a refused zone;
  - a reorder that dips into the content area less than `PANE_DROP_DEPTH`
    and still reorders, as a regression check of today's rule, plus a
    release outside the window that still reorders.

  Escape, a pane drag released outside the window, a release on the source
  pane, and a blur each leave every layout and the hierarchy sequence
  unchanged. No shell receives a byte during any drag, checked with the
  ordered ACK.

### Palette and arguments (criterion 4)

- Tier 2: each argument command, run once through the palette's pickers
  and once through an explicit invocation, produces equal canonical
  layouts. The destinations include another workspace and another session.
- Palette unit tests:
  - the pane picker's rows, order, and exclusion of the focused pane;
  - the side picker;
  - `StaleTarget` for a pane that vanished while the picker stayed open.
- Keymap and schema: `side` validates and appears as an enum, and `pane`
  cannot be set in a binding.

### Undo issue

- Protocol and core, for `restore_tabs`:
  - undo and redo of every #29 operation return the exact earlier state;
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
- Client history unit tests:
  - two and three consecutive operations on one tab undo and redo in full;
  - an external change to a tab cuts only the entries chained through it,
    and an entry on another tab survives;
  - depth eviction drops the oldest entry at 51;
  - a new operation clears the redo stack;
  - pruning reaches a window that does not show the touched tab;
  - pruning defers while the window's own operation, undo, or redo is in
    flight, so the operation's event cannot prune the previous top before
    the completion records the new entry.
- Tier 2: undo of a tab-into-pane drop reopens the tab at its index, with
  its name and `TabId`, focuses the entry's pane, and keeps every view
  entity in the window; a refused undo shows one notice; `cmd-z` with the
  palette open edits nothing.

### Native smokes

- `smoke:macos-input` checks exact PTY bytes. It drags a title bar with
  AppKit events (`mouseMoved:`-based motion, as the divider drag does) onto
  a sibling's edge and onto its centre, then drags a tab from a top bar
  onto a pane. It asserts:
  - the new layout in smoke state;
  - a shell ACK from every pane, and the same `$$` per pane;
  - the ordered no-byte check for every shell.
- `smoke:linux-input` runs the production binary without smoke state, as
  the split panes deviations record, so the Linux drags live in
  `check-overlays.ts`. Both palette smokes run it with AppKit or XTest
  input. It covers the same drops, Escape cancelling a pane drag, and the
  no-byte check.
- `check-viewers.ts` panes step: a move between tabs through `invoke-in`,
  with the second window adopting the pane and its shell answering.
- Smoke state gains `w<i>.pane_tabs=` (each pane's tab) and the drag target
  (`w<i>.drop=`), set during paint, so a smoke waits for the drawn overlay
  before releasing.
- Native coverage uses a top tab bar on both platforms. The other three
  positions and both styles rely on tier 2, which drives the same
  `TabStrip` geometry.
- Run the host macOS smokes and `mise run linux:smoke` in Docker. Check
  `df -h` first and clean volumes and images after. Never run
  `vm:macos:smoke`.

### Commands

- Iterate with `mise run check`.
- Run focused tests: `cargo test -p huterm-protocol`,
  `cargo test -p huterm-core mux::`, and
  `cargo test -p huterm-gpui --features test-support`.
- Run `mise run test:ghostty-releasesafe` for PRs that add core PTY tests.
- Run `mise run verify` before each PR leaves draft.
- Run `mise run bench:idle` on macOS before and after the drag PR, and
  confirm it shows no added idle wakeups.

## Staging

Four PRs, each leaving `main` consistent and shippable:

1. **#29 PR 1, runtime moves.** `with_inserted`, `with_swapped`,
   `SelfTarget`, `PanesRearranged` and its reducer, `swap_panes`,
   `move_pane`, `move_tab_to_pane`, `LayoutChange` results, the revision
   rules, the allocation preflight, and protocol and core tests, including
   PTY tests. The desktop handles the event through its existing
   drop-and-install path, and no command triggers it yet.
2. **#29 PR 2, commands.** The argument kinds, pickers, commands, and menus;
   reconcile adoption and pane order; the focus and activation rules; Pane
   close targets that follow their pane; `cleanup_spawn` sparing moved-in
   panes; and the tier 1, tier 2, palette, and `check-viewers.ts` tests.
   Users can move and swap panes by keyboard and palette.
3. **Undo issue PR.** `restore_tabs`, the per-window history and its
   pruning pass, `undo` and `redo` with bindings and menus, and the undo
   tests.
4. **#29 PR 3, drags.** `drop_zone`, pane and tab drags, the reorder
   exception, the overlay, the tab bar target, and the native smokes.

Undo goes in a separate issue rather than inside #29 (open question 1). It
has its own acceptance criteria, and #30's presets and balancing record into
it as well. Landing it before the drags means a tab-into-pane mis-drop never
ships without a one-key recovery. If Jim prefers to keep it inside #29, the
same PR becomes the third #29 PR and the order is unchanged.

## Documentation

- `CONTEXT.md`: **Layout history** (a window's bounded record of the
  layout rearrangements it made, which undo and redo restore).
- `docs/agents/core-runtime.md` (PR 1 and the undo PR):
  - terminal conservation;
  - one event per rearrangement;
  - the allocation preflight;
  - swaps leaving the revision alone;
  - restore's guard, and its reuse of a removed tab's ID;
  - "moves retain empty parents" narrowed to workspaces and sessions.
- `docs/agents/desktop-client.md` (PR 2): reconcile adoption, pane order,
  and Pane close targets that follow their pane.
- `docs/agents/desktop-input.md` (PR 3): pane and tab drags, the overlay,
  cancellation, and the reorder exception for pane mode.
- `docs/keybindings.md`: the new commands, `undo` and `redo` bindings, and
  the Linux binding notes.
- This plan's `Status:` in each PR.

## Non-goals

- Any drag across windows, and tear-out: #31.
- Hovering a tab label during a pane drag to switch to that tab
  ("spring-loaded" tabs; open question 3).
- Undoing splits, closes, renames, tab opens, focus, or zoom.
- Pane custom names, persisting history, or history shared between windows.

## Open questions

1. **Undo staging.** Recommend a separate issue under #12, landing between
   #29's command PR and its drag PR. Alternative: undo as #29's third PR.
2. **Default bindings for move and swap.** Recommend none in #29: the
   obvious candidates collide with platform conventions (`cmd-shift-[` and
   `]` switch tabs in macOS apps) or with Linux desktop shortcuts
   (older GNOME releases bind `ctrl-alt-shift-<arrow>` to moving windows
   between workspaces), and Ghostty ships no pane moves at all. Users can
   bind them. Alternative: `cmd-shift-alt-<arrow>` and
   `ctrl-super-alt-<arrow>` for directional swaps.
3. **Spring-loaded tabs.** Recommend deferring: dragging a pane over a tab
   label for 600 ms would activate that tab so the pane can land in its
   panes. Commands already reach any tab.
4. **Recording divider resizes.** Recommend not recording them (see
   "What is recorded"). Alternative: record each committed drag and
   coalesce consecutive resize commands on one split within one second.
5. **Linux undo keys.** Recommend `ctrl-shift-z` and `ctrl-shift-y`.
   Alternative: `ctrl-super-z` and `ctrl-super-shift-z`, matching the
   `ctrl-super` pane focus keys.
6. **Restored tab identity.** Recommend reusing the removed tab's `TabId`.
   Alternative: a fresh ID on each undo, which keeps "an ID never returns"
   as an invariant at the cost of rewriting history entries after each
   restore.
7. **History owner.** Recommend window-owned history, which dies with its
   window, including a window that a move of its own emptied. Alternative:
   history owned by the runtime per attachment. It survives an IPC client
   reconnecting, but it still dies with the attachment, and it needs a way
   to publish availability for menus.
8. **`cmd-z` with nothing to undo.** Recommend taking the key always, for
   now (see "Commands, bindings, menus"). Revisit with Kitty keyboard
   protocol support, which would let a terminal program read the key.

[issue-12]: https://github.com/jimeh/huterm/issues/12
[issue-29]: https://github.com/jimeh/huterm/issues/29
[ghostty-config]: https://github.com/ghostty-org/ghostty/blob/ebd449995d321e90d6c73be3decb15558512b2ed/src/config/Config.zig
[ghostty-binding]: https://github.com/ghostty-org/ghostty/blob/ebd449995d321e90d6c73be3decb15558512b2ed/src/input/Binding.zig
