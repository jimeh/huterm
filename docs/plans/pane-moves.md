# Pane moves

Status: in progress. Approved on 2026-10-26 after a Claude review and four
Codex review rounds, then revised with Jim's settled decisions and two more
Codex review rounds. PR 1 (runtime moves) is in progress. Plans
[issue #29][issue-29] within [epic #12][issue-12]. Builds on [the split panes
plan](split-panes.md), which fixed the drop zone geometry, the title bar as
a drag handle, and its settled decisions 1 to 3 for this issue, cited here
as "split panes decision N". Layout undo is [#256][issue-256], a related
issue with [its own plan](layout-undo.md) and no ordering dependency
(settled decision 1).

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
- Nothing activates a tab during a drag. Reorder never changes the active
  tab, and the window's earliest-deadline timer (`refresh::Animated`)
  drives deadlines such as notice expiry from an injected `now`.

## Scope across #29, #30, and #31

| Issue | Owns |
| --- | --- |
| #29 | Swap and move commands (next, previous, four directions); moving a pane beside a pane in any tab on the runtime; extracting a pane into a new tab; moving a tab into a pane; the pane and tab drags inside one window, the drop overlay, dropping a pane on the window's own tab bar, and spring-loaded tabs; the atomic multi-tab hierarchy event; returning each change's before and after tab records |
| #30 | Presets, cycling, and balancing |
| #31 | Every drag that crosses a window boundary: a tab or pane dropped on another window's tab bar or panes, and tear-out into a new window. It reuses #29's overlay, `drop_zone`, and Mux operations. |

Issue #29's drags end at their own window. A pane drag released outside
the window, or over another window, cancels and leaves every layout
unchanged. A tab drag released there keeps today's reorder behavior and
reorders within its own bar (see "Drags"). #31 later turns those releases
into transfers and tear-outs.

Layout undo ([#256][issue-256]) is related but separate: [its
plan](layout-undo.md) builds on this plan's first PR, and #29 neither waits
for it nor depends on it.

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
- **Mis-drops are hard to make and easy to fix.** A tab reorder becomes a
  drop only past a deliberate depth, the overlay previews every drop's
  result before release, and the move and extract commands reverse any
  drop by keyboard or palette.

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
  that now holds it, it flattens into that split (split panes decision 3) with
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
  (split panes decision 3).

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
  a `Placed` change (see "Hierarchy events"), so every move is one atomic
  event of one kind.
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

Every operation returns a `LayoutChange`: each touched tab's record before
and after, read under the lock, and empty for a no-op.

```text
LayoutChange { before: Vec<TabSnapshot>, after: Vec<TabSnapshot> }
TabSnapshot  { tab: TabId, record: Option<TabRecord> }       None: absent
TabRecord    { session, workspace, index, custom_name, layout, zoomed }
```

The protocol and core tests assert expected results against it. The layout
undo plan records it.

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
- a tab dropped into a pane: `Removed` and `Layout`.

The layout undo plan's restores emit the same event.

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

No default bindings (settled decision 2): users can bind any of these
commands. Menus:

- Terminal context menu: Move Pane to New Tab, shown in split tabs.
- macOS Window menu and the Linux window menu: Move Pane to New Tab.
- Tab menu: Move Tab into Pane…, which opens the palette on that command
  with `tab` filled.
- Everything else is in the palette.

### Drags

Two new drags join the window's capture listeners, beside reorder and
divider drags. Like them, they own motion and release once started, stop
propagation, and never reach a terminal. They keep terminal focus, so no
application sees a focus report, unless spring-loading switches tabs (see
"Spring-loaded tabs").

**Pane drag.** A left press on a title bar's title area, not its buttons,
focuses the pane as today; motion past `TAB_DRAG_THRESHOLD` starts a
`PaneDrag { tab, pane, origin, pointer, target }`. Title bars exist only in
split tabs (split panes decision 1), so a lone pane is dragged from the tab bar
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
  `drop_zone(frame, point)`. The four edge zones are always offered. The
  centre is offered only when the target pane is in the source pane's tab,
  since `swap_panes` works within one tab (see "Spring-loaded tabs").
- For a tab drag in pane mode: the four edge zones. Its centre offers
  nothing (split panes decision 2).
- For a pane drag over the tab bar: an insertion slot, which extracts the
  pane into a new tab there, or an inactive tab's spring zone (see
  "Spring-loaded tabs"). Overflow autoscroll works as it does for reorder.
- For a tab drag outside pane mode: today's reorder slot.
- For a pane drag anywhere else, including outside the window: no target.

**Overlay.** Drawn above the panes and below menus and dialogs:

- an edge zone tints the half of the target frame the dropped pane or
  layout would occupy;
- the centre zone outlines the whole target frame and the source pane, the
  two that swap;
- a tab bar insertion slot draws the reorder insertion marker, and an armed
  spring zone outlines its label;
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
- a tab switch the drag did not make by spring-loading;
- a close request;
- an overlay opening;
- a layout change that removes the source pane or tab.

A tab drag released outside pane mode reorders, as today. A refused
operation, such as a target that vanished between the frame and the
commit, shows one notice. A drop is an explicit request, unlike a divider
drag's silent snap-back. A pane drag that spring-loaded switches back when
it cancels or is refused (see "Spring-loaded tabs").

### Spring-loaded tabs

A pane drag that rests on an inactive tab's label activates that tab after
a short delay, so the pane can be dropped into that tab's panes (settled
decision 3). Without it, a pane drag reaches only the tab it started in and
the tab bar's insertion slots.

**Delay: `SPRING_LOAD_DELAY`, 500 ms, a constant.** For reference:

- Chromium's `HoverTabSelector`, which its header calls "spring-loaded" tab
  transitions, waits `kHoverTransitionDelay`, 500 ms
  ([hover_tab_selector.cc][chromium-hover] on `main` at `07350d3d`, read on
  2026-10-26). `BrowserTabStripController::OnDropIndexUpdate` starts it only
  for a drop hovering directly over a tab, not between tabs
  ([browser_tab_strip_controller.cc][chromium-strip]).
- Firefox selects a hovered tab after `browser.tabs.dragDrop.selectTab.delayMS`,
  350 ms ([firefox.js][firefox-prefs]), and only for link drags
  ([drag-and-drop.js][firefox-dnd]).
- macOS spring-loading, as in Finder, is a system setting
  (`com.apple.springing.enabled` and `com.apple.springing.delay`). Apple
  documents a speed slider but no default value. The machine this plan was
  written on reads 0.5 s.

500 ms matches Chromium and the observed macOS value, and sits at the fast
end of the 0.5 to 1 second range. Firefox's 350 ms is faster, but a pane
drag that crosses the bar on its way to an insertion slot would trip it more
often. A constant keeps Linux, which has no system equivalent, on the same
behavior, and a setting is cheap to add later. Whether macOS should follow
its system spring-loading setting is open question 2.

**Which drags: pane drags only.** A tab drag over another tab's label is a
reorder. Spring-loading there would switch tabs in the middle of a reorder,
and because a tab drag nests only into the active tab's panes, the switch
would also turn a plain reorder into a possible nest. Chromium and Firefox
both reserve hover-to-activate for external drags such as links and files,
and never spring-load during a tab drag. To nest tab A into tab B, the user
activates B first, then drags A.

**Zones on a label.** For a pane drag, `TabStrip` splits each label along
the bar's axis, horizontal or vertical:

- the middle half of an inactive tab's label is that tab's spring zone;
- the outer quarters of every label, the gaps between labels, and the bar
  past the last tab are insertion slots, from `TabStrip::slot`, which
  extract the pane into a new tab there;
- the middle half of the active tab's label is no target.

This follows Chromium's split between hovering a tab and hovering between
tabs.

**Arming and firing.** Entering a spring zone arms `SpringLoad { tab,
armed_at }`. Moving into another label's spring zone re-arms for that tab,
and leaving to anything else disarms. While armed, the window reports
`armed_at + SPRING_LOAD_DELAY` through its `animation_schedule`, as one
deadline on the window's earliest-deadline timer. It requests no frames,
so a pointer resting on the label still fires and waiting costs one timer.
`advance_animation(now)` fires once `now` reaches the deadline. It
re-resolves the pointer against the last drawn strip:

- If the hit result is still the armed tab's spring zone, and that tab is
  still in the window and inactive, the window activates it. Being over
  the same label is not enough: a resize or a title change can leave a
  still pointer over that label's insertion quarter.
- If the hit result is another tab's spring zone, the window re-arms for it
  with a fresh `armed_at`.
- Otherwise it disarms.

Overflow autoscroll moves labels under a still pointer and so re-arms each
time; a scrolling bar never springs. A drag whose pointer is already over a
spring zone when it crosses the drag threshold arms at that moment, as if
it had just entered.

**Activation.** Spring-loading activates the tab through the existing
select path, as a click would: the window model's active tab changes, the
tab's pane views show and resize, and keyboard focus moves to its focused
pane, so focus reports follow the real switch. The source pane's tab hides
under the hidden-view rules, and the drag continues because its state lives
in `WorkspaceView`. The window records `origin_tab`, the tab active when
the drag started, and `sprung_tab`, the last tab spring-loading activated.
Spring-loading is the drag's own tab switch. Any other switch, from a key
binding, a click, or reconcile, still cancels the drag. Resting on the
origin tab's label springs back to it.

**Dropping into a spring-loaded tab.** Over that tab's panes, the pane drag
is offered the four edge zones, which move the pane beside the target with
`move_pane` and `Beside`, a cross-tab move. Centre eligibility follows the
tab, not the spring: the centre zone is offered only when the target pane
is in the source pane's tab, because `swap_panes` exchanges panes within one
tab (open question 1). A drag that springs from A to B and back to A is
offered A's centre zones again. After a drop, the destination tab stays
active and the moved pane takes focus, under the initiating window's rule
in "Client reconciliation".

**Cancellation and switching back.** A drag that cancels, for any reason
listed under "Release" in "Drags", or whose drop the runtime refuses,
switches back when it spring-loaded and the active tab is still
`sprung_tab`:

- the window reactivates `origin_tab`, if it still exists, through the
  same select path;
- keyboard focus returns to that tab's focused pane, which is the dragged
  pane, since a title-bar press focuses its pane, unless an overlay holds
  focus. A drag cancelled because a palette, menu, or dialog opened
  switches back without taking focus from it, following reconcile's
  existing activation rule for a window whose overlay holds focus.

The switch was only a means to reach a drop target, so a drag that drops
nothing leaves the user where the gesture began, with the dragged pane
focused. If something else switched tabs, the drag already cancelled and
nothing switches back. If `origin_tab` is gone, the active tab stays.

Releasing on a spring zone, armed or already activated, is a release with
no target: it cancels and switches back. Escape before the delay disarms
and cancels; after it, Escape cancels and switches back. A release outside
the window cancels and switches back, as for any pane drag.

**Feedback.** While armed, the hovered label draws in the drop-target
style: the accent outline from `TabColors`. It is static, with no progress
animation, so waiting draws one frame and arms one timer. Once the tab
activates, it takes the active style, and the overlay targets its panes.

### `drop_zone`

`drop_zone(frame, point) -> Option<Zone>`, with `Zone = Edge(SplitDirection)
| Centre`, lives in `desktop/windows/pane_geometry.rs` as the split panes
plan fixed it: the outer quarter of the frame, title bar included, on each
side is that edge's zone; where two edge zones overlap near a corner, the
edge nearer the pointer as a fraction of the frame's width or height wins,
ties going to the top or bottom edge; the rest is the centre. A point
outside the frame has no zone.

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
| The armed tab closes, or moves to another workspace, before the delay | The deadline handler finds it gone from the window and disarms; nothing activates | Tier 2 |
| Another client reorders the armed tab before the delay | At the deadline the label under the pointer belongs to another tab, so the window re-arms for that tab, or disarms | Tier 2 |
| The spring-loaded tab closes while the drag hovers its panes | Reconcile's removal rule picks the most recently active tab, normally `origin_tab`. That switch was not the drag's, so the drag cancels and nothing more switches back | Tier 2 |
| The origin tab closes, or the dragged pane leaves it, during a spring-loaded drag | The drag cancels because its source changed; with `origin_tab` gone, nothing switches back | Tier 2 |
| The dragged pane exits while another tab is spring-loaded | As for any pane drag: an exit-close that removes it cancels the drag, which switches back if the origin tab remains | Tier 2 |
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
- Mis-drop recovery in #29: a tab reorder becomes a drop only past
  `PANE_DROP_DEPTH`, the overlay shows each drop's result before release,
  and the move and extract commands put any pane or tab back by keyboard or
  palette.
- Spring-loading needs a deliberate 500 ms rest on a label, shows an
  outline while it waits, and switches back if the drag ends without a
  drop.
- The tab bar target uses `TabStrip`, which already handles all four
  positions, both styles, and overflow. Tier 2 covers every position, and
  the native smokes cover the top bar. Panes as small as the minimum keep
  their four edge zones by fraction.

## Testing and verification

Mapped to #29's acceptance criteria. Apply the global testing rules:

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
  - a pane moved into a pending spawn's tab before `cleanup_spawn` runs.

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
  - stale and gap rules hold.
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

### Spring-loaded tabs (criterion 3)

Time is controlled, never slept. `SpringLoad` is a pure model that takes
every `Instant` from its caller, as `NoticeStack` does, and the window's
`refresh::Animated` implementation receives `now` as an argument. The tests
read `armed_at` from the model and call `advance_animation` with instants
derived from it.

- Unit tests:
  - the `SpringLoad` model arms, re-arms on another tab, and disarms;
  - its deadline is `armed_at + SPRING_LOAD_DELAY`;
  - it is due at the deadline and not one millisecond before;
  - `TabStrip`'s label zones, for all four positions: the middle half of an
    inactive label is a spring zone, the active label's middle is no
    target, and outer quarters, gaps, and the space past the last tab are
    insertion slots.
- Tier 2, through `Window::dispatch_event` and synthetic instants:
  - **Timing:** a pane drag resting on an inactive label is armed, and
    `animation_schedule` reports exactly the deadline and no frame;
    `advance_animation` at `armed_at + 499 ms` activates nothing, and at
    `armed_at + 500 ms` activates the tab and moves keyboard focus to its
    focused pane;
  - **Re-arming:** moving to another label before the deadline re-arms
    with a new `armed_at`; leaving the bar disarms, and the schedule goes
    idle; a drag that starts over a spring zone arms at the threshold;
  - **Geometry without motion:** a resize or title change that leaves the
    still pointer over the armed label's insertion quarter activates
    nothing at the deadline, and one that leaves it over another label's
    spring zone re-arms;
  - **Autoscroll:** a pane drag resting at the overflow edge scrolls the
    bar, re-arms as labels pass, and activates nothing while scrolling;
  - **Dropping:** a drop on an edge zone of the spring-loaded tab's pane
    moves the pane across tabs, keeps that tab active, and focuses the
    pane; its centre zone offers nothing; a drag that springs from A to B
    and back to A swaps on a sibling's centre;
  - **Switching back:** Escape, a release on a spring zone, a release
    outside the window, and a refused drop each reactivate `origin_tab`
    with the dragged pane focused; a drag cancelled by an opening palette
    switches back and leaves focus in the palette;
  - **Other switches:** a tab switch by key binding during the drag cancels
    it and switches nothing back;
  - **Tab drags:** a tab drag resting on another label past the deadline
    activates nothing and still reorders on release;
  - **Races:** each spring-loading row in "Races". Rows about the armed tab
    apply their event before the deadline is driven; rows about the
    spring-loaded or origin tab apply it after activation;
  - **No input leaks:** no shell receives a byte, checked with the ordered
    ACK.

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
- Spring-loading, on both platforms (`smoke:macos-input` with AppKit events,
  `check-overlays.ts` with XTest on Linux):
  - drag a title bar onto another tab's label and hold the pointer still;
  - wait, with a bounded deadline, until smoke state reports that tab
    active;
  - move onto an edge zone of one of its panes and release, then assert the
    moved pane's tab, a shell ACK from every pane, and the same `$$`;
  - repeat to the switch, then press Escape, and assert the origin tab is
    active again and every layout is unchanged.

  The real timer is the integration boundary here, so the smoke polls state
  rather than sleeping a fixed time. Tier 2 owns the timing assertions.
- `check-viewers.ts` panes step: a move between tabs through `invoke-in`,
  with the second window adopting the pane and its shell answering.
- Smoke state gains `w<i>.pane_tabs=` (each pane's tab), the drag target
  (`w<i>.drop=`), and the armed spring tab (`w<i>.spring=`), each set during
  paint, so a smoke waits for the drawn overlay before releasing.
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
  confirm it shows no added idle wakeups. An armed spring load adds one
  deadline only during a drag.

## Staging

Three PRs, each leaving `main` consistent and shippable:

1. **PR 1, runtime moves.** `with_inserted`, `with_swapped`, `SelfTarget`,
   `PanesRearranged` and its reducer, `swap_panes`, `move_pane`,
   `move_tab_to_pane`, `LayoutChange` results, the revision rules, the
   allocation preflight, and protocol and core tests, including PTY tests.
   The desktop handles the event through its existing drop-and-install
   path, and no command triggers it yet.
2. **PR 2, commands.** The argument kinds, pickers, commands, and menus;
   reconcile adoption and pane order; the focus and activation rules; Pane
   close targets that follow their pane; `cleanup_spawn` sparing moved-in
   panes; and the tier 1, tier 2, palette, and `check-viewers.ts` tests.
   Users can move and swap panes by palette and their own bindings.
3. **PR 3, drags.** `drop_zone`, pane and tab drags, the reorder exception,
   the overlay, the tab bar target, spring-loaded tabs, and the tier 2 and
   native tests.

Layout undo ships separately, on its own schedule (settled decision 1).
Its runtime PR needs this plan's PR 1 on `main`, and its client PR needs
PR 2. No PR here waits for it.

## Documentation

- `docs/agents/core-runtime.md` (PR 1):
  - terminal conservation;
  - one event per rearrangement;
  - the allocation preflight;
  - swaps leaving the revision alone;
  - "moves retain empty parents" narrowed to workspaces and sessions.
- `docs/agents/desktop-client.md` (PR 2): reconcile adoption, pane order,
  and Pane close targets that follow their pane.
- `docs/agents/desktop-input.md` (PR 3): pane and tab drags, the overlay,
  cancellation, the reorder exception for pane mode, and spring-loading.
- `docs/keybindings.md` (PR 2): the new commands, unbound by default.
- This plan's `Status:` in each PR.

## Non-goals

- Any drag across windows, and tear-out: #31.
- Layout undo: [its own plan](layout-undo.md).
- Spring-loading during tab drags (see "Spring-loaded tabs").
- Pane custom names.

## Settled decisions

Jim decided these on 2026-10-26, after the first round of reviews. Each item
names the alternatives he considered.

1. **Undo is split out completely.** Layout undo is its own issue,
   [#256][issue-256], and [plan](layout-undo.md), with no ordering
   dependency: #29 does not wait for it, and its drag PR does not depend on
   it. #29 keeps only the hooks
   it needs on its own, the `Placed` change and the `LayoutChange` results.
   Not chosen: undo as a #29 PR, or a separate issue landing before #29's
   drag PR.
2. **No default bindings for move and swap.** The commands are in the
   palette, and users can bind them. The obvious keys collide with
   platform conventions (`cmd-shift-[` and `cmd-shift-]` switch tabs in
   macOS applications) or with desktop shortcuts (older GNOME releases bind
   `ctrl-alt-shift-<arrow>` to moving windows between workspaces). Not
   chosen: `cmd-shift-alt-<arrow>` and `ctrl-super-alt-<arrow>` for
   directional swaps.
3. **Spring-loaded tabs are in #29**, in the drag PR, as "Spring-loaded
   tabs" specifies. Not chosen: deferring them, with the move commands as
   the only way to reach another tab.

## Open questions

1. **The centre zone in a spring-loaded tab.** Recommend offering nothing
   on a pane outside the source pane's tab, as for tab drags, so
   `swap_panes` stays a single-tab operation. A drag that springs back to
   the source pane's tab gets its centre zones again.
   Alternative: a cross-tab swap, which sends the target pane back into the
   hidden origin tab. That would honor split panes decision 2 literally, but
   it widens `swap_panes` to two tabs, makes swaps advance the close
   revision, and moves a pane the user cannot see.
2. **macOS system spring-loading setting.** Recommend the 500 ms constant on
   both platforms. Alternative: on macOS, read `com.apple.springing.enabled`
   and `com.apple.springing.delay` so Huterm follows the user's
   accessibility choice, at the cost of native defaults reads and a second
   code path that Linux cannot share.

[issue-12]: https://github.com/jimeh/huterm/issues/12
[issue-29]: https://github.com/jimeh/huterm/issues/29
[issue-256]: https://github.com/jimeh/huterm/issues/256
[chromium-hover]: https://chromium.googlesource.com/chromium/src/+/07350d3d4eeb10bc22254989d9fd58b9199b09d7/chrome/browser/ui/tabs/hover_tab_selector.cc
[chromium-strip]: https://chromium.googlesource.com/chromium/src/+/07350d3d4eeb10bc22254989d9fd58b9199b09d7/chrome/browser/ui/views/tabs/browser_tab_strip_controller.cc
[firefox-prefs]: https://searchfox.org/firefox-main/source/browser/app/profile/firefox.js
[firefox-dnd]: https://searchfox.org/firefox-main/source/browser/components/tabbrowser/content/drag-and-drop.js
