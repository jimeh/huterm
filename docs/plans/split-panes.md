# Split panes

Status: in progress. Approved on 2026-10-26 after a Claude review and two
Codex review rounds; PR 1 (the runtime split tree) is in progress. Plans
[issue #28][issue-28] within [epic #12][issue-12], and sets the pane UX that
[#29][issue-29] and [#30][issue-30] build on.

A tab holds a split layout of panes, and each pane shows one terminal. This
is the tmux, iTerm2, and Ghostty model, not VS Code's editor groups. It
completes the ownership chain in the [roadmap][issue-8]: socket, sessions,
workspaces, tabs, pane layouts, terminals. The split structure and its
proportions are runtime state that every view shares. Which pane has focus,
and which pane is zoomed, belong to each view.

Dependencies [#20][issue-20], [#22][issue-22], and [#23][issue-23] have
landed, and so have [#97][issue-97] and [#98][issue-98], which this plan
extends.

## Current state

Observed on `main` at `5662bb47`:

- `huterm_core::Tab` (`crates/huterm-core/src/mux.rs`) holds one `pane_id`,
  one `terminal_id`, a custom name, and the launched program's
  `fallback_name`. Its doc comment says "Splits can replace this leaf
  later." `PaneId` is an unscoped `opaque_id`; `Tab::info` and the digest
  copy it, and no behavior depends on it.
- `TabInfo` (`crates/huterm-protocol/src/hierarchy.rs`) mirrors that record,
  and `HierarchyEvent` (`#[non_exhaustive]`) has no pane events. The digest
  hashes the tab's pane and terminal IDs. `insert_tab` validates the tab's
  runtime scope, its workspace, and the position, but nothing checks that
  pane or terminal IDs are unique across tabs.
- Every Mux path that maps tabs to terminals reads `tab.terminal_id`:
  `subscribe_terminal`'s membership check, cross-session move revocation,
  `close_tab`, `close_workspace`, `close_session`, and `prepare_close`.
- `Mux::revision` advances on every structural change, renames included, and
  a close commit whose ticket revision moved returns `StaleClose`. The
  desktop then assesses again and shows the confirmation again whenever the
  jobs still need consent (`CloseState::checked` in
  `crates/huterm-gpui/src/desktop/windows.rs`).
- The desktop keeps one `TerminalView` per `TabView`. Installs,
  replacements, exit-closes (`Desktop::exit_closes`), exit notices, close
  targets, and the window model's `TabEntry.terminal` are keyed by tab.
  `WorkspaceView::active_view` has about 40 callers that treat the active
  tab's view as the window's terminal: Terminal-scope commands, the palette
  target, the inherited directory, and focus restoration among them.
- `TerminalView` computes its own content bounds from `ChromeLayout`, using
  copies of the tab configuration, sidebar width, presentation, insets,
  notch shelf, and window frame. `sync_tab_layout`, `resize_sidebar`, the
  fullscreen sync, view creation, and Quake each push those copies. The
  scrollbar's track margins also read them, so the scrollbar avoids a
  right-hand tab bar's corner. Inside the bounds, `TerminalLayout` computes
  the requested grid and `GridGeometry` maps the pointer onto the displayed
  grid.
- Tab labels follow `[tabs] label` and `[tabs] directory`
  (`resolve_tab_label`): by default a running program's title or name, else
  the directory, else the terminal title, else the launched program name.
- New tabs already inherit a directory: `[terminal] new_tab_directory =
  "inherit"` (the default) reads the active view's
  `TerminalMetadata::directory`, which core fills from OSC 7 or the probed
  foreground or root process directory. Remote OSC 7 paths are rejected,
  and an unusable directory falls back to the default launch directory:
  home on macOS, the process's current directory on Linux.
- Tab reordering uses window-level capture listeners and its own
  `TabReorder` state, not GPUI's active drag. The preview stays constrained
  to the bar.
- The renderer paints every cursor the same way; it has no focus input.
- The catalog already has a `zoom` command, which zooms the window. Pane
  zoom needs another ID.
- `cmd-w` (macOS) and `ctrl-shift-w` (Linux) close the tab, and the File,
  window, tab, and terminal menus show that shortcut beside Close Tab.

## Scope across #28, #29, and #30

The split Jim proposed holds, with one addition: #28 also defines the pane
hit-test regions, the title bar layout, and the drop-zone geometry contract,
so #29 adds gestures without reworking them.

| Issue | Owns |
| --- | --- |
| #28 | Runtime split tree and pane and split IDs; split, close, resize, zoom, and navigation commands; divider drags; layout-derived bounds for rendering, input, scrollbars, and PTY sizes; pane close through consent tickets; the pane title bar with title, zoom, and close; per-view focus and zoom |
| #29 | Pane move and swap commands; extracting a pane into a tab and moving it into another tab; dragging a pane by its title bar; dropping a tab from the tab bar onto a pane; the drop-zone overlay |
| #30 | Named presets, cycling, and balancing |
| #31 | Unchanged. Tab transfer already carries the whole layout, because the layout lives on the tab. |

Proposed issue edits, for Jim to make or approve:

- **#28:** add "Each pane in a split tab shows a title bar with its title, a
  zoom toggle, and a close button" to Scope, and replace the acceptance line
  on native input with "Native multi-pane input, selection, divider drag,
  resize, and title bar controls work on macOS and Linux."
- **#29:** add to Scope: "Drag a pane by its title bar onto another pane's
  edge or centre zone, or onto the tab bar; drag a tab from the tab bar onto
  a pane's zones. Drops resolve through the same move and swap operations as
  the commands." Add the drop-zone and cancellation cases to the acceptance
  tests.
- **#12:** no change; its scope already lists moving and transfers.

### Staged delivery of #28

Issue #28 is too large for one reviewable PR. Four PRs, each leaving `main`
consistent. The first two change nothing a user sees:

1. **PR 1, runtime split tree.** Protocol layout types and events, the
   reducer, Mux operations, pane close tickets, and core PTY tests. The
   desktop adapts to the new records and keeps showing one pane per tab; it
   offers no split command yet.
2. **PR 2, pushed terminal geometry.** `TerminalView` stops computing its
   own chrome and receives its geometry from `WorkspaceView` through one
   function. Every view still fills the tab, so nothing moves. Verified by
   the existing fullscreen, Quake, title-row, and client-frame smokes.
3. **PR 3, desktop panes.** Per-pane views and viewers, the layout function,
   dividers, pane title bars, split, close, and navigation commands with
   default bindings and menu items, view-local focus, focused-pane routing,
   the tab label rule, per-pane exit-close, and their tests and smoke steps.
4. **PR 4, resize and zoom.** Divider drags, resize commands, and
   view-local zoom, with native smoke coverage of divider drags.

Title bars ship with the first splits, in PR 3, because they change every
split pane's size; adding them later would change PR 3's pane geometry,
PTY sizes, and size assertions. A release can be cut between PRs, so PR 3
must be a usable feature on its own: split, focus by click or key, close by
key or title bar, and a visible focused pane.

Jim settled every product question on 2026-10-26 (see "Settled
decisions"), so no stage waits on a decision.

## Principles

The [hierarchy projection](hierarchy-projection.md) and
[terminal viewers](terminal-viewers.md) principles apply: reactive and never
polled, idle costs nothing, work scales with the change, correctness never
depends on observing one event, and the runtime is the only arbiter of
terminal size. This plan adds:

- **One layout function.** Pane rectangles come from one pure function of
  the layout, the tab's content bounds, the view's zoom, the view's
  in-progress divider drag, and the cell metrics. Painting, hit testing,
  scrollbars, and requested grids all read its output; nothing recomputes
  bounds on its own.
- **Structure is shared, presentation is local.** Splits, closes, and
  committed proportions go through the runtime and reach every view through
  the hierarchy stream. Focus, zoom, hover, and an in-progress divider drag
  never leave the view.
- **A single pane looks and behaves exactly as a tab does today.**

## Decisions

### The layout is an n-ary split tree in `huterm-protocol`

```text
PaneLayout
  root: PaneNode
PaneNode = Pane(PaneInfo) | Split(SplitNode)
SplitNode { id: SplitId, axis: SplitAxis, children: Vec<SplitChild> }
SplitChild { weight: u32, node: PaneNode }
PaneInfo { id: PaneId, terminal_id: TerminalId, fallback_name: String }
SplitAxis = Columns | Rows       side by side, or stacked
SplitDirection = Left | Right | Up | Down
```

N-ary rather than binary, as tmux's layout cells are: splitting a pane along
its parent's axis inserts a sibling, so three equal columns are one node, and
[#30][issue-30]'s balancing and presets work on siblings directly.

Invariants, checked by one `PaneLayout::validate` that core and the reducer
both call:

- Every split has at least two children.
- No split has a child split on the same axis; those are flattened.
- Each split's weights are at least 1 and sum to exactly `SPLIT_TOTAL`
  (1,000,000). Integer weights with a fixed sum keep equality and the digest
  exact, and need no floats in the contract.
- A split has at most `MAX_SPLIT_CHILDREN` (256) children, so its largest
  child always holds at least `SPLIT_TOTAL / 256` and can absorb any
  rounding (see "Remove").
- Pane, split, and terminal IDs are unique within the layout.

The transforms live beside the types, so core, the reducer, and tests share
one implementation:

- **Split** a pane toward a direction. When the pane's parent splits on that
  direction's axis, the new pane becomes its sibling, before it for Left and
  Up and after it for Right and Down, and the two divide the pane's weight,
  the original keeping the larger half. Otherwise the pane is replaced by a
  new split of two equal children in its slot. Splitting a lone root creates
  a root split. A pane whose weight is 1, or whose parent already holds
  `MAX_SPLIT_CHILDREN`, cannot be split.
- **Remove** a pane. Its weight goes to its previous sibling, or to the next
  one when it was first, so only one neighbour grows, as in tmux. A split
  left with one child is replaced by that child, which takes the split's
  weight. A child split that now shares its new parent's axis is flattened
  into it: each grandchild's weight becomes `floor(weight × slot /
  SPLIT_TOTAL)`, raised to 1 if that is 0. The merged split's weights then
  differ from `SPLIT_TOTAL` by at most its child count; the difference is
  added to, or taken from, the merged split's largest child, the first among
  equals, which the child limit keeps far above 1. Reapportioning the whole
  merged split, not only the slot, keeps a weight-1 slot valid: its
  grandchildren may end up larger than the slot was, and their siblings
  shrink by the same amount. A flatten that would exceed
  `MAX_SPLIT_CHILDREN` is skipped, leaving the same-axis child in place;
  `validate` accepts that only at the limit. Removing the last pane is not a
  layout operation (see "Closing a pane").
- **Set weights** of one split, by `SplitId`, given the expected child
  identities in order (pane or split IDs). A different arrangement, such as
  one #29 swapped, fails even with the same child count.
- **Traversal order:** depth-first, children in order. It defines next and
  previous pane and the first pane.
- **Normalized geometry:** each pane's rectangle in `u128` units, 2^96
  across the root on each axis, derived from weights alone, ignoring
  pixels, zoom, and minimum sizes. A split at `[start, start + extent)`
  places child `i`'s far edge at `start + floor(extent × prefix_i /
  SPLIT_TOTAL)`, where `prefix_i` is the sum of the weights up to and
  including child `i`; the product stays below 2^116. Neighbours share that
  boundary value, so adjacency and distances compare exactly. Extreme
  nesting can still give a pane zero extent on an axis; such a pane is never
  a directional candidate, and navigating from it uses its degenerate
  rectangle. Directional navigation uses this geometry, so the candidates
  and distances are the same in every view whatever its size; only the
  final tie-break, this view's focus history, is view-local.

`PaneId` becomes a `scoped_id`, like `TabId`, and `SplitId` is a new
`scoped_id`, so Mux rejects foreign targets as it does for tabs. `TerminalId`
stays opaque.

### Hierarchy events carry the whole tab layout

`TabInfo` becomes `{ id, custom_name, layout, zoomed }`; `pane_id`,
`terminal_id`, and `fallback_name` move into `PaneInfo`, and `zoomed:
Option<PaneId>` is the tab's shared zoom (see "Zoom scope"). Two new events:

```text
PaneLayoutChanged { tab, layout, zoomed }   split, pane close, resize
PaneZoomChanged   { tab, zoomed }           shared zoom set or cleared
```

`PaneLayoutChanged` carries the resulting shared zoom, so a close that
removes the zoomed pane clears the zoom in the same event.

A tab's layout is a handful of nodes, so sending all of it keeps the event
state-like and the reducer a validated replacement. It also covers
[#29][issue-29]'s swaps and moves within one tab and [#30][issue-30]'s
presets without new events. A pane moving between tabs (#29) must change
both tabs atomically, so #29 adds a `PaneMoved` event rather than two
layout events that a drain could split. Closing a tab's last pane emits
`TabClosed`.

The reducer applies `PaneLayoutChanged` under the existing rules: a stale or
duplicate sequence is ignored, a gap or foreign stream asks for a resync,
and an event naming an unknown tab, an invalid layout, a pane or terminal
another tab holds, or a shared zoom naming a pane outside the layout is
`Inconsistent`. `TabOpened` and
`HierarchyState::from_sessions` run the same validation and cross-tab check,
since they carry layouts too. The state keeps an index from pane and from
terminal to their tab, so the cross-tab check and the client's
terminal-to-pane lookups are constant time.

`Touched` gains the set of tabs whose layout or shared zoom changed. Those
tabs are also
added to `tabs`, because a tab's label can change with its panes, and to
their workspace's entry, so a pane install that failed retries on the next
layout change of its tab as a tab install retries on a workspace change.

The digest's tab record becomes `b'T'`, tab ID, custom name, the shared zoom
as an optional pane ID (`0`, or `1` and the ID), then the layout in
traversal order: `b'P'`, pane ID, terminal ID, fallback name for a pane;
`b'X'`, split ID, axis (`0` columns, `1` rows), child count, and each
child's weight before that child, for a split. The documented encoding and
the golden-value test change with it.

### Core owns the tree and spawns before it commits

`huterm_core::Tab` keeps `id` and `custom_name` and holds a `PaneLayout`.
Mux gains, each taking the expected workspace like `close_tab`:

- `split_pane(workspace, tab, pane, direction, command) ->
  Result<OpenedPane>`. It validates the targets, checks that the pane can be
  split, spawns the terminal, and only then inserts the pane and emits. A
  failed spawn changes no structure and emits nothing; like `open_tab`, it
  still consumed identities.
- `close_pane(workspace, tab, pane)`. It removes the pane, emits
  `PaneLayoutChanged`, then stops and joins the pane's runtime. Emission
  precedes the fallible cleanup, as for `close_tab`. The last pane closes
  the tab through `close_tab` (settled decision 7).
- `resize_split(workspace, tab, split, children, weights)`. Equal weights
  are a no-op that emits nothing.
- `set_pane_zoom(workspace, tab, zoomed: Option<PaneId>)` sets or clears
  the tab's shared zoom and emits `PaneZoomChanged`. Setting the current
  value is a no-op. A pane outside the tab fails with `UnknownPane`. A lone
  pane cannot be zoomed and fails with `InvalidLayout`. `split_pane` and
  `close_pane` keep it consistent: closing the zoomed pane clears it, and
  splitting a tab clears it, matching the view rule that splitting unzooms.
- `select_pane(pane) -> SelectionTarget`, which gains a `pane` field, and
  `Tab::terminals()`, which replaces every `tab.terminal_id` read listed
  above.
- New errors: `UnknownPane`, `UnknownSplit`, `InvalidLayout` (wrong
  children or weights), and `PaneTooSmall`.

Splits and pane closes call `changed()`, which invalidates pending close
tickets: they change which terminals a ticket covers. `resize_split` and
`set_pane_zoom` do not. Neither weights nor zoom change a ticket's scope,
and a stale commit makes the desktop show its confirmation again, so a
divider drag in one window would otherwise bounce another window's Confirm
click back to the same dialog.
`Mux::revision` gains a doc comment (it has none today): it advances on
every change that can affect a close ticket's scope. The hierarchy sequence
still advances for resizes and zoom changes.

Focus is never core state, and zoom is core state only in shared mode. The
terminal viewer contract already handles a pane whose views differ in size
(see "PTY sizes"). Shared zoom ships in PR 1 because it is part of the tab
record, the events, and the digest; adding it in PR 4 would reopen all
three. No client calls `set_pane_zoom` until PR 4.

### Closing a pane uses the existing consent tickets

`CloseRequest::Pane { workspace, tab, pane }` resolves under the Mux lock:

- a missing or foreign target fails as tab targets do;
- the tab's only pane resolves to `CloseEffect::Tab`, so the last pane
  closes the tab through the existing tab path, including the emptied-window
  close from [the session close plan](session-close.md) (settled decision 7);
- otherwise `CloseEffect::Pane`, whose ticket covers that pane's terminal
  only.

`CloseRequest::Tab` and `Tabs` cover every pane's terminal, so closing a
split tab with a job in any pane prompts. Assessment, consent, staleness,
and the two-second age limit are unchanged. A split committed between a tab
close's assessment and commit makes the commit stale, and the retry covers
the new pane.

On the desktop, PR 3 adds `CloseTarget::Pane(PaneId)`. A Pane target merges
like a Tab target: a wider target absorbs it, and two different pane closes
queue. Its confirmation reads "Close pane?" and names the pane's title. A
tab or window confirmation for a split tab groups its jobs under pane
titles, "tab › pane", instead of one tab heading, so two busy panes are
distinguishable. `TabEntry` gains a terminal-to-pane map for those headings
in place of its single `terminal`. `remove_closed_tabs` gains a pane
counterpart that drops the pane view and moves focus (see "One view per
pane").

An exited pane follows the tab rules per pane:

- With `close_on_exit = true`, the pane closes and its siblings take the
  space. The last pane's exit closes the tab, then the window, as today.
  `Desktop::exit_closes` is keyed by terminal instead of tab. Its `retain`
  runs on layout changes as well as tab removals, and `holds` checks that
  the window shows the pane. A pane's exit can arrive before its
  `PaneLayoutChanged`; the record waits, owed, until a window shows the
  pane, as an exit seen before a tab install does today. The one-actor rule
  is unchanged.
- With `close_on_exit = false`, the exited pane stays with its history and
  its exited state, in its title bar and, while it has focus, the tab label.
  Its exit notice names the pane.

### The desktop keeps one view per pane

`TabView` holds its panes: `PaneView { pane, terminal, view:
Entity<TerminalView>, activity_task }`, one viewer per pane. Installs and
replacements (`view_install.rs`, `projection::installs` and
`replacements`) work per pane: reconcile derives the panes the projection
holds in the window's shown tabs without a view, and the panes whose views
were revoked. A pane the projection no longer holds is dropped, and the
`committed`-sequence rule for removals applies per pane. A split's own
completion pushes its view, as a spawn does, and holds `busy` until then.

View-local state lives in the window model's `TabEntry`, because a future
restore needs it and window commands read it: `focused: PaneId`, `zoomed:
Option<PaneId>`, and the tab's pane focus history, most recent first. When
the focused pane disappears, focus moves to the most recently focused pane
still present, else the first pane in traversal order. A zoomed pane that
disappears clears the zoom.

A view's effective zoom is its own `zoomed` in view mode and the tab's
shared zoom in shared mode. Whenever the effective zoom names a pane other
than the view's focused pane, which happens when another window sets a
shared zoom, the view moves its focus to the zoomed pane, since a hidden
pane cannot hold focus, and records it in the focus history. Commands and
focus restoration then follow the zoomed pane. Moving GPUI focus inside an
inactive window reports no focus to the terminal, so a remote zoom never
takes size control away from the active window. After unzoom, focus stays
on the pane that was zoomed.

Focused-pane routing: `active_view` resolves to the active tab's focused
pane, so every caller that means "the window's terminal" follows focus:
Terminal-scope commands and their `StaleTarget` check, the palette's target
terminal, the inherited directory, focus restoration after menus and
dialogs, and smoke state. Callers that mean the tab, such as the tab bar,
take the `TabView`. Each caller is classified in PR 3, not left to the
default.

Moving focus between panes moves GPUI focus between their views, so each
view's `enqueue_focus` reports focus out and in to its terminal, with focus
reporting on, exactly as switching windows does. A press in an unfocused
pane's terminal focuses that pane explicitly in its mouse-down handler,
because the terminal calls `prevent_default`, which stops `track_focus`.

### Pane geometry is computed once and pushed to views

PR 2 replaces `TerminalView`'s chrome fields (`tabs_config`,
`sidebar_width`, `chrome_hidden`, `fullscreen_insets`, `notch_shelf`,
`window_frame`, `tab_presentation`) with one pushed value:

```text
TerminalGeometry {
    bounds,           terminal content bounds in window coordinates
    track_margins,    scrollbar track insets, from the tab bar position
    quiet,            the change is chrome appearing or hiding, not a resize
}
```

`WorkspaceView::terminal_geometry` computes it, from `ChromeLayout` at
reveal progress 0.0 as `content_bounds` does today, and every site that
pushes the old fields (`sync_tab_layout`, `resize_sidebar`, the fullscreen
sync, view creation, and Quake) calls it. `content_bounds`, `viewport`,
`terminal_layout`, `grid_geometry`, and the scrollbar geometry read the
pushed value, so painting, the pointer, scrollbars, and the requested grid
share it, as the desktop guide requires. `tab_overlay` stays separate. In
PR 2 every view's bounds equal today's terminal bounds.

PR 3 adds a pure module, `desktop/pane_geometry.rs`, that computes for one
tab:

- each pane's frame, title bar (when shown), and terminal bounds;
- each scrollbar's track margins: a pane at the tab's right edge keeps the
  tab bar corner rule, others get none;
- each divider's line, its grab zone, its split, and the child boundary it
  sits on; and
- with a zoomed pane, that pane's frame filling the tab and no dividers.

Inputs: the layout, the tab content bounds from `TerminalGeometry`, the
view's zoom and drag preview weights, the cell size, and the minimum pane
size. Dividers are one logical point wide, and boundaries round to physical
pixels. Space shorter than the minimum sizes shrinks every pane
proportionally down to one cell, so a small window or a second view never
refuses to draw a layout. `WorkspaceView` paints dividers and title bars
from the same output the last render used, and the terminal views paint
inside their pushed bounds.

The minimum pane size is 4 columns by 2 rows (settled decision 9), measured
as the grid `TerminalLayout` would compute inside each pane's terminal
bounds. `TerminalLayout` clamps a grid to one cell, so it does not enforce
the minimum; the commands do. A split preflights the whole candidate layout
in this view, unzoomed, with title bars, dividers, and padding, and refuses
when any pane would fall below the minimum. The first split of a tab adds a
title bar to both panes, so a short tab can refuse a left or right split.
Divider drags and resize commands clamp so no pane falls below it. The
proportional shrink above applies only to space another view or a window
resize imposes, never to a split this view accepted.

Padding (`[window] padding_x`, `padding_y`) applies inside each pane, as
`TerminalLayout` already applies it inside the terminal bounds.

### PTY sizes, focus, and zoom follow the viewer contract

Each pane view reports its requested grid through its own viewer, as tab
views do now. Under `latest`, the viewer that most recently gained focus or
typed controls a terminal's size. Panes add no rule of their own:

- Moving focus between panes in one window changes no terminal's controller
  unless another window also shows that terminal.
- A zoomed pane's view requests the zoomed grid, so while that view
  controls, the PTY takes the zoomed size. The tab's other panes are hidden
  in that view and keep requesting their split geometry, the hidden-tab rule
  today, so zooming resizes only the zoomed pane's terminal.
- Another window showing the same tab unzoomed displays the canonical grid
  clipped, or with background, until the user focuses or types there, which
  takes control and resizes the PTY to that window's pane. This is #22's
  existing behavior for two windows of different sizes.
- [#207][issue-207]'s `smallest`, `largest`, and `manual` policies need no
  change for panes: they aggregate viewers, and each pane view is one
  viewer.

Zoom scope is configurable (settled decision 4). `[panes] zoom = "view"`,
the default, keeps zoom in the view's `TabEntry` as above. `[panes] zoom =
"shared"` uses the tab's runtime zoom from PR 1 (`TabInfo::zoomed`,
`set_pane_zoom`, and `PaneZoomChanged`): a client in shared mode sends its
zoom commands to the runtime and shows the tab's shared zoom; a client in
view mode ignores it. PR 4 adds the setting and the client side. Sizing is
the same in both modes: each view that shows the pane
zoomed requests the zoomed grid, and `latest` picks the controller.

Hiding a pane for zoom applies the hidden-view rules from the input guide:
release accepted application gestures, finalize the selection, forget
cancelled buttons, and ignore stale pointer callbacks. Zooming cancels a
divider drag.

### Rendering and invalidation

Each pane view keeps its own renderer, row cache, and snapshot, as each tab
view does. The existing rules carry over:

- A geometry change marks geometry pending and runs `resize_to_layout`,
  which reports the new grid. Until the runtime's snapshot at the new
  geometry revision arrives, painting clips the old snapshot to the new
  bounds, and `GridGeometry` maps the pointer and selection onto the
  displayed grid. Cached rows are reused only from the snapshot, never
  repositioned for the new bounds.
- Focus, zoom, divider moves, selection, and title bar state notify the
  views whose bounds or presentation changed. They never advance a
  terminal's content generation and never request a snapshot by themselves;
  a snapshot follows only when the requested grid changed and the view
  controls.
- A focus change repaints the cursor and both title bars. The renderer
  paints every cursor the same way today, so PR 3 adds a `focused` flag to
  `TerminalRenderer::paint`: a pane view without keyboard focus in a split
  tab draws a block cursor as a one-point outline, as Ghostty and iTerm2 do
  for unfocused splits. The flag is presentation only; it changes no
  prepared row. A divider drag repaints dividers and both neighbours.

### Input and hit testing

Inside a tab's content bounds, in priority order:

1. **Overlays.** The palette, menus, the close dialog's scrim, toasts, the
   scroll pill, and a revealed fullscreen tab overlay keep their own press
   rules. Nothing below claims a press while one of them is under the
   pointer or a modal shows.
2. **Divider grab zones.** Each zone is 8 points across the divider line,
   clipped to the two panes' frames. It wins over a pane's overlay
   scrollbar, which yields the 4 points next to an inner divider; at a
   tab's outer edge the scrollbar keeps its place. `WorkspaceView`'s
   window-level capture listeners, registered before the terminal views as
   tab reordering's are, claim a left press inside a zone and stop
   propagation, so no terminal sees it and no selection or application mouse
   report starts. This avoids the sidebar handle's failure, where a grab
   zone straddling the terminal started a terminal gesture. With padding
   under 4 points the zone overlaps grid cells next to the divider; those
   cells stay reachable by keyboard and by pressing a pixel further in. A
   hitbox element over each zone sets the resize cursor, which a capture
   listener cannot.
3. **Title bars,** when shown. A press focuses the pane and never reaches
   the terminal. The zoom and close buttons act on release over the same
   button, like tab close buttons. The title area is the drag source #29
   adds.
4. **Pane terminal bounds.** Existing terminal handling, unchanged apart
   from the explicit focus on press above. A press in an unfocused pane is
   delivered too, as the click that activates a window is today.

A terminal gesture in progress keeps ownership of motion and release beneath
dividers and title bars, as gestures do beneath overlays. Hit tests run
against the geometry of the last drawn frame, so a layout change between
frames cannot hand a press to the wrong pane.

### Divider drags commit once, on release

A divider drag keeps preview weights in the view and feeds them to the
layout function, so the two panes move under the pointer at once. On
release, the view commits the weights with `resize_split` on a worker,
passing the split's expected children. The commit does not hold `busy`; it
is a single Mux operation that blocks nothing else. The view keeps showing
the preview until the projection reaches the commit's sequence, then drops
it, so the divider never snaps back. A rejected commit drops the preview
silently and the view shows the projection's layout. Escape, blur, a tab
switch, hiding, zoom, closing the window, and a layout change that removes
the split or changes its children cancel the drag and drop the preview.
Resize commands commit the same way, once per command.

The preview also resizes the two PTYs during the drag (settled decision
10), through the normal geometry path, as a window resize does: the views'
requested grids follow the preview, so shells reflow while dragging, and
other windows see the new proportions only after release.

### The pane title bar

A 24-point row at the top of each pane frame, shown only in split tabs
(settled decision 1):

```text
[status] title ....................... [zoom] [close]
```

- **Status:** the same indicator a tab shows for its terminal: running dot,
  bell, exited, or failed.
- **Title:** what the tab would show if this were its only pane: the tab
  label resolution, `resolve_tab_label` under `[tabs] label` and
  `[tabs] directory`, applied to this pane's terminal and fallback name,
  without the tab's custom name. Truncated with an ellipsis.
- **Zoom:** a toggle, highlighted while zoomed. Hidden until PR 4 ships
  zoom.
- **Close:** closes the pane through its consent ticket.
- Colours come from `TabColors`: the focused pane uses the active tab colours
  and accent, the others the inactive ones. Controls use the hover and
  `control_pressed` styles the desktop guide requires.
- The row reserves no space for a drag grip. #29 makes the title area the
  drag source and may add a grip on hover, which shifts nothing.

### Drop zones, for #29

This plan fixes the geometry so that #29 only adds the gesture and the
overlay. Over a pane's frame, title bar included:

- the edge zones are the outer quarter of the frame on each side; where two
  overlap near a corner, the edge nearer the pointer, measured as a fraction
  of the frame's width or height, wins, with ties going to the top or bottom
  edge;
- the centre zone is what remains;
- dropping on an edge splits that side of the pane with the dropped pane or
  layout; the centre's effect is settled decision 2, and a dropped multi-pane
  tab is settled decision 3.

The function `drop_zone(frame, point) -> Zone` will live in
`desktop/pane_geometry.rs`; #29 implements it with its tests, since nothing
in #28 calls it.

### Commands, bindings, and menus

Separate commands per direction, so palette labels are unambiguous and
bindings need no enumerated argument kind:

| Command | Scope | macOS | Linux |
| --- | --- | --- | --- |
| `split_right`, `split_down` | Window | `cmd-d`, `cmd-shift-d` | `ctrl-shift-o`, `ctrl-shift-e` |
| `split_left`, `split_up` | Window | none | none |
| `close_pane` | Window | `cmd-w` | `ctrl-shift-w` |
| `focus_next_pane`, `focus_previous_pane` | Window | `cmd-]`, `cmd-[` | `ctrl-super-]`, `ctrl-super-[` |
| `focus_pane_left`, `_right`, `_up`, `_down` | Window | `cmd-alt-<arrow>` | `ctrl-alt-<arrow>` |
| `resize_pane_left`, `_right`, `_up`, `_down` (optional `amount`, cells, default 5) | Window | `cmd-ctrl-<arrow>` | `ctrl-super-shift-<arrow>` |
| `toggle_pane_zoom` | Window | `cmd-shift-enter` | `ctrl-shift-enter` |

The bindings follow Ghostty's and iTerm2's defaults, and none collides with
a current default. `close_pane` takes over `cmd-w` and `ctrl-shift-w`; on a
single-pane tab it closes the tab, so behavior is unchanged until the user
splits. `close_tab` stays in the catalog and palette, unbound by default
(settled decision 6). `docs/keybindings.md` documents the `super` modifier, which
GPUI accepts.

Menus: the macOS File menu and the Linux window menu gain Split Right, Split
Down, and Close Pane; the terminal context menu gains Split Right, Split
Down, and Close Pane; the tab menu keeps Close Tab. Shortcut text follows
the effective bindings, so Close Tab shows none by default.

Behavior:

- **Split** inherits the focused pane's directory under `new_tab_directory`
  (its description gains "and new panes"), resolved when the command runs,
  with today's fallback (settled decision 8). It refuses with a notice when the
  focused pane, in this view, cannot hold two panes of the minimum size along
  that axis. Splitting a zoomed tab unzooms first. The new pane takes focus.
- **Next and previous** follow traversal order and wrap.
- **Directional focus** never wraps. Candidates lie wholly beyond the focused
  pane's edge in the normalized geometry: their near edge is at or past it.
  The nearest near edge wins; among equal distances, a candidate whose span
  on the perpendicular axis overlaps the focused pane's by a positive length
  beats one that only touches or misses it; then the longer overlap; then
  the pane this view focused most recently; then traversal order. With no
  candidate, nothing happens.
- **Resize** moves the focused pane's edge in that direction through the
  nearest ancestor split on that axis where the pane has a neighbour on that
  side, clamped to the minimum sizes, then commits as a divider drag does.
- **Navigation while zoomed** unzooms and then focuses the target, as tmux
  does.

Command availability: pane commands are unavailable while the window is
busy or shows a close confirmation. Focus, resize, and zoom are unavailable
on a tab with one pane.

### Tab labels and icons

The tab label resolves the tab's custom name, else the focused pane's title
in this window. The status indicator is the focused pane's, except that a
bell or failure in any pane also shows on the tab, because they ask for
attention. Each window resolves its own label, so two windows focused on
different panes of one tab can label it differently; the window model's
published titles already belong to their window (settled decision 5).

The palette's tab rows keep using every open window's published titles, as
now; for a tab no window shows, they use its first pane's title.

## Races

| Race | Handling | Evidence |
| --- | --- | --- |
| A split commits between a tab close's assessment and commit | The revision moved: `StaleClose`, reassess, the new pane is covered | Core lifecycle test |
| A resize commits while another window's close confirmation shows | Resizes leave the revision alone, so the confirmation's commit stays valid | Core lifecycle test |
| A pane close commits before another window's split of that pane | The split fails with `UnknownPane` and changes nothing; that window shows a notice | Core test; tier 2 |
| A split commits before another window's close of the original pane | The original pane survives a split, so the close still applies to it; an assessment taken before the split is stale and reassesses, then closes only that pane | Core lifecycle test; tier 2 |
| Another window sets a shared zoom on a pane this view does not focus | The view moves focus to the zoomed pane; an inactive window reports no focus | Tier 2, shared and mixed modes |
| A resize computed against a layout another window changed | `resize_split` rejects an unknown split or different children; the window drops the preview silently and shows the new layout | Core test; tier 2 |
| A divider drag while another window changes the same tab | The drag cancels when its split disappears or its children change; otherwise it continues from the new weights | Tier 2 |
| The focused or zoomed pane closes in another window | Focus moves to the most recently focused remaining pane; zoom clears | Tier 2 |
| A `PaneLayoutChanged` applies before the split's completion | The completion waits on its committed sequence, as spawns do; reconcile never installs the pane a busy window is splitting | Tier 2 |
| A split fails to spawn | No structure changed and no event; the window shows a notice and keeps focus | Core and tier 2 |
| A pane exits while its close confirmation shows | The exit-close and the confirmation belong to the same window pipeline; one actor, as for tabs | Tier 2 |
| A pane's exit arrives before its layout event | The owed exit-close waits until a window shows the pane | Tier 2 |
| A layout event and a pane's terminal events interleave | Terminal events for a pane the projection no longer holds are ignored, as for tabs | Existing rule, tier 2 |

## Testing and verification

Mapped to #28's acceptance criteria. Apply the global testing rules: each
new test must fail at its assertion before its fix, or under one targeted
perturbation for tests over existing behavior.

### Repaint without content changes (criterion 1)

- Tier 1 (`desktop/view_tests.rs`), PR 3 and PR 4: with no PTY output after
  the first snapshot, a focus move, a selection, a zoom toggle, and a divider
  move each repaint with the same `Arc<TerminalRow>` values, request no
  snapshot, and change the cursor, selection, or bounds as expected.
- Tier 1: shrinking a pane clips the retained snapshot to the new bounds
  until the new geometry revision arrives; hit tests and selection use the
  displayed grid throughout.
- `pane_geometry` unit tests: bounds, track margins, dividers, grab zones,
  zoom, drag previews, and the short-space fallback.
- PR 2: existing tier 1 and tier 2 tests pass unchanged, and a unit test
  shows `terminal_geometry` equals the old `content_bounds` for each tab
  position, presentation, and frame.

### Trees, navigation, sizes, failures, closure, and viewers (criterion 2)

- Protocol unit tests (PR 1): split in each direction on a lone root, on the
  parent's axis, and across it; removal giving weight to the previous or
  next sibling, collapsing a one-child split, and flattening a same-axis
  child with the weight floor and remainder, including a promoted
  two-child split landing in a weight-1 slot; the child limit refusing a
  split and skipping a flatten; weight setting with wrong children and
  invalid sums; `validate` on each invariant; traversal order and
  normalized geometry on an uneven nested tree and on extreme nested
  weights; directional neighbours at edges, with positive perpendicular
  overlap winning, corner contact losing, zero-extent panes never chosen,
  and each documented tie-break.
- Reducer tests (PR 1): `PaneLayoutChanged` and `PaneZoomChanged` apply with
  the expected `Touched`; unknown tabs, invalid layouts, duplicate pane or
  terminal IDs, and a zoom naming a pane outside the layout are
  `Inconsistent`, from `PaneLayoutChanged`, `PaneZoomChanged`, `TabOpened`,
  and `from_sessions`; stale and gap rules hold; the digest changes with
  weights, axis, order, and zoom.
- Core tests (PR 1): the differential reducer test drives split, pane close,
  last-pane close, resize, and shared zoom, including closing the zoomed
  pane and splitting a zoomed tab; rejected and no-op operations emit
  nothing; a failed split spawn leaves structure, sequence, and terminal
  count unchanged; foreign pane and split IDs are rejected.
- Core lifecycle tests (PR 1): `CloseRequest::Pane` resolves to `Pane` or,
  for the last pane, `Tab`; a busy pane prompts and an idle sibling does
  not; a tab close prompts for a job in any pane; a split between
  assessment and commit makes it stale; a resize or a shared zoom between
  them does not; a split committed before a pane close's commit makes it
  reassess and then close only the original pane.
- Tier 2 (`windows/window_tests.rs`), PR 3 and PR 4: two windows on one
  session focus and zoom different panes of one tab independently; a split
  in one window installs a view in the other; closing a pane elsewhere moves
  focus; minimum-size refusal of a split, including a first split at the
  height boundary where the title bars would push both panes under two
  rows; Terminal-scope commands and the palette target follow pane focus;
  pane close confirmations and their headings; shared zoom set in one
  window moving the other window's focus, in shared and mixed modes; the
  races table's tier 2 rows.

### PTY survival and sizes (criterion 3)

- Core PTY tests (PR 1): split a tab running a real shell; both shells
  answer; closing one pane reaps its child (`kill(pid, 0)` returns `ESRCH`)
  while the sibling still answers; closing the tab reaps both. Each pane's
  viewer reports a distinct geometry and each shell's `stty size` reports its
  own. Synchronize on shell ACK output, never sleeps.
- Tier 2 (PR 3): after a split, each pane's shell reports the rows and
  columns that `pane_geometry` computes for it; after a close, the survivor
  reports the merged size.

### Native multi-pane input (criterion 4)

- PR 2: `smoke:macos-fullscreen`, `smoke:macos-quake`, `smoke:macos-palette`
  (title row), and the Linux fullscreen, quake, palette, and client-frame
  smokes pass unchanged.
- PR 3: a `panes` step in `scripts/check-viewers.ts`, which runs in
  `smoke:macos-palette` and `smoke:linux-palette`, splits through catalog
  commands, types into each pane through `type-in`, and checks per-pane
  `stty size` against new `w<i>.panes=` smoke state. `type-in` bypasses
  native focus, so `smoke:macos-input` and `smoke:linux-input` also click
  an unfocused pane natively, type, and require that pane's own shell
  ACK; enable application mouse tracking in one pane and check its reported
  cell coordinates; click a title bar's close button; and dispatch the new
  default keys, asserting that none reaches the shell.
- PR 4: the input smokes gain a divider drag with native pointer events
  (`mouseMoved:`-based motion on macOS, XTest on Linux), asserting new pane
  sizes in smoke state and a shell ACK in each pane, a selection drag
  inside one pane that stops at its bounds, and a native click on a title
  bar's zoom button.
- PR 3: `mise run bench:idle` on macOS with a three-pane tab, before and
  after, shows no added idle wakeups.
- Run the host macOS smokes and `mise run linux:smoke` in Docker, checking
  `df -h` first and cleaning volumes and images after. Never run
  `vm:macos:smoke`.

### Commands

`mise run check` while iterating, focused `cargo test -p huterm-protocol`,
`cargo test -p huterm-core mux::`, and `cargo test -p huterm-gpui --features
test-support`, confirming new test names in the runner output.
`mise run test:ghostty-releasesafe` for PRs that add core PTY tests, and
`mise run verify` before each PR leaves draft. PR 2, PR 3, and PR 4 also run
`mise run ci:benchmarks` to confirm one-pane output latency and keystroke
budgets are unchanged.

## PR 1 implementation steps

1. `huterm-protocol`: make `PaneId` scoped, add `SplitId`, a `pane_layout`
   module with the types, `validate`, the transforms, traversal, normalized
   geometry, and directional neighbours; change `TabInfo`, including the
   shared zoom; add `PaneLayoutChanged` and `PaneZoomChanged`; extend the
   state's indexes, `Touched`, the reducer, and the digest.
2. `huterm-core`: hold a `PaneLayout` and shared zoom in `Tab`; add
   `split_pane`, `close_pane`, `resize_split`, `set_pane_zoom`,
   `select_pane`, and the errors; replace `terminal_id` reads with
   `Tab::terminals()`; add `CloseRequest::Pane` and `CloseEffect::Pane`;
   exempt resizes and zoom from the revision and document the revision.
3. `huterm-gpui`: adapt to the changed records. Each tab view shows the
   tab's first pane, labels use that pane's fallback name, and no command
   splits. A layout with several panes cannot arise from the desktop in this
   PR; until PR 3 the desktop would show only the first pane of such a tab.
4. Tests listed above for PR 1, then `mise run verify` and
   `mise run test:ghostty-releasesafe`.

## Documentation

- `CONTEXT.md`: **Split** (a node of a tab's pane layout that divides its
  space among child panes or splits along one axis) and **Pane zoom** (a
  view showing one pane of a split tab across the whole tab, without
  changing the shared layout).
- `docs/agents/core-runtime.md`, Mux section: the layout invariants, spawn
  before commit, the last-pane rule, and resizes leaving the close revision
  alone (PR 1).
- `docs/agents/desktop-client.md` and `desktop-input.md`: pushed terminal
  geometry (PR 2); per-pane installs, exit-closes, focused-pane routing, and
  divider capture (PR 3 and PR 4).
- `docs/keybindings.md` and `docs/configuration.md`: the commands, bindings,
  the `super` modifier, and `new_tab_directory` applying to panes (PR 3).
- `crates/huterm-gpui/src/desktop/windows/model.rs`: the module comment's
  "pane layouts (#28)" note (PR 3).
- This plan: update `Status:` in each PR.

## Non-goals

- Pane moves, swaps, extraction, and every drag gesture onto panes: #29.
- Presets, cycling, and balancing: #30.
- Pane custom names. A tab keeps one custom name.
- Dimming unfocused panes or a focus border beyond the title bar colours
  and the outlined cursor.
- Persisting layouts: #42 captures them through `capture_hierarchy` once it
  writes anything.

## Settled decisions

Jim decided these on 2026-10-26, after the plan's first review. Each item
names the alternatives he considered.

1. **Title bar on a single-pane tab:** shown only while a tab is split, so
   an unsplit tab looks as it does today. A lone pane is its tab, so #29
   drags it from the tab bar. Not chosen: always shown, or a setting.
2. **Centre drop (#29):** a pane dropped on another pane's centre swaps the
   two; a tab dropped on a pane's centre does nothing, and the overlay
   offers it only the four edges. Not chosen: replace, or nothing for both.
3. **A multi-pane tab dropped onto a pane (#29):** its whole layout nests at
   the chosen side, keeping its proportions and pane IDs; a same-axis join
   flattens. Not chosen: flatten its panes, or refuse.
4. **Zoom, focus, and canonical size:** a view that shows a pane zoomed
   requests the zoomed grid for that pane only, siblings keep their split
   size, and `latest` decides the controller, as under "PTY sizes". Zoom is
   view-local by default, and `[panes] zoom = "shared"` makes it runtime
   state seen by every client in shared mode. Not chosen: zoom that never
   resizes the PTY.
5. **Tab title and icon with several panes:** they follow this window's
   focused pane, and a bell or failure in any pane also shows on the tab.
   Not chosen: always the first pane, or a pane-count badge.
6. **Default bindings:** the Ghostty-style set in the commands table, with
   `cmd-w` and `ctrl-shift-w` moving to `close_pane`. Not chosen: the Linux
   alternatives that avoid Xfce workspace and IBus emoji keys, or keeping
   `cmd-w` on `close_tab`. The Linux conflicts stay documented in
   `docs/keybindings.md`, so users know what to rebind.
7. **Closing the last pane:** it closes the tab through the tab's consent
   ticket, then the window as today. Not chosen: refusing it.
8. **New pane directory:** splits follow `new_tab_directory`, inheriting the
   focused pane's directory with today's fallback. Not chosen: a separate
   setting, or always the default directory.
9. **Minimum pane size:** 4 columns by 2 rows. Not chosen: 10 by 3.
10. **Divider drag preview:** shells reflow live during the drag, and other
    windows see the result on release. Not chosen: reflow on release.
11. **Scope and staging:** the issue table, the proposed issue edits, and
    the four-PR staging stand, and this delivery ships PR 1.

## Open questions

None.

[issue-8]: https://github.com/jimeh/huterm/issues/8
[issue-12]: https://github.com/jimeh/huterm/issues/12
[issue-20]: https://github.com/jimeh/huterm/issues/20
[issue-22]: https://github.com/jimeh/huterm/issues/22
[issue-23]: https://github.com/jimeh/huterm/issues/23
[issue-28]: https://github.com/jimeh/huterm/issues/28
[issue-29]: https://github.com/jimeh/huterm/issues/29
[issue-30]: https://github.com/jimeh/huterm/issues/30
[issue-97]: https://github.com/jimeh/huterm/issues/97
[issue-98]: https://github.com/jimeh/huterm/issues/98
[issue-207]: https://github.com/jimeh/huterm/issues/207
