# Pane layouts

Status: approved on 2026-10-27, after five Codex and five Claude review
rounds; PR 1, runtime arrangements, is in progress. Jim settled every open
question that day (see "Settled decisions"), adding default cycle bindings
and user-defined layouts. Plans [issue #30][issue-30] within
[epic #12][issue-12].
Builds on [the split panes plan](split-panes.md) (#28) and
[the pane moves plan](pane-moves.md) (#29), both implemented. Layout undo,
[#256][issue-256], is related with no ordering dependency; this plan assumes
[its plan](layout-undo.md) lands first, with all seven of its
recommendations approved, and records layouts and balancing through it.

A user can rearrange a tab's existing panes into a named layout, either one
of five built-ins (equal columns, equal rows, main left, main top, and
tiled) or one they define in the config file or the Settings window, step
through a list of layouts with next and previous commands, or balance the
current split tree without changing its shape. None of this creates,
closes, or restarts a terminal, and no pane changes identity: only the
tab's split tree and its weights change.

## Current state

Observed on `main` at `f0b0174c`:

- `PaneLayout` (`crates/huterm-protocol/src/pane_layout.rs`) is an n-ary
  tree whose splits hold integer weights summing to `SPLIT_TOTAL`
  (1,000,000, `:18`), with at most `MAX_SPLIT_CHILDREN` (256, `:23`)
  children and no child split on its parent's axis. It validates on
  construction (`:216`). `panes()` returns traversal order, depth first
  with children in order (`:228`); `cycle` (`:475`) and `step_target`
  (`:501`) use it. `same_shape` (`:386`) compares splits, axes, child
  counts, and pane placement, ignoring split IDs and weights.
  `with_weights` (`:422`) changes one split. Nothing builds a whole new
  tree from existing panes.
- `Mux::resize_split` (`crates/huterm-core/src/mux.rs:627`) sets one
  split's weights, refuses changed children, emits `PaneLayoutChanged`
  through `emit_layout` (`:723`), treats equal weights as a no-op, and
  leaves `Mux::revision` alone (`:166-168`). `allocate` advances the
  revision before it bumps `next_id` (`:232-237`). `split_pane` clears the
  shared zoom (`:580`).
- `Mux::swap_panes` and the within-tab moves (`mux/moves.rs:62`, `:175`)
  return a `LayoutChange` of one tab, emit `PaneLayoutChanged`, and refuse
  through an allocation preflight (`:421`) before any change. Within-tab
  moves clear the shared zoom (`:205`); swaps keep it. `snapshot` (`:474`)
  reads a tab's `TabRecord`.
- `LayoutChange`, `TabSnapshot`, and `TabRecord` live in
  `crates/huterm-protocol/src/hierarchy.rs:180-220`. The reducer applies
  `PaneLayoutChanged` through `replace_layout` (`:990`), which validates
  the layout and cross-tab uniqueness, and marks the tab in
  `Touched::layouts` (`:446`).
- `desktop/windows/pane_geometry.rs` lays out a tab in one pure function,
  `layout_panes` (`:72`). Each split's last child ends at the far edge and
  the others snap to physical pixels (`place`, `:214-262`); a split tab
  gives every pane a 24-point title bar (`:264-290`).
- The minimum pane is 4 columns by 2 rows (`windows.rs:81-82`).
  `pane_grids` (`:6486`) computes each pane's grid with `TerminalLayout`,
  and `fits_minimum` (`:153`) checks it. A split preflights its candidate
  layout with these (`pane_moves.rs:173` builds candidates with a
  placeholder split ID). `clamp_resize` (`:4577`) bisects a boundary
  against `keeps_minimum` (`:91`).
- Resize commands and divider drags commit through one ordered per-window
  queue (`commit_resize`, `windows.rs:4754-4798`) without holding `busy`.
  The window shows committed weights (`committed_layout`, `:5370`) until
  the projection holds them. Move commands commit on a worker with `busy`
  set (`pane_moves.rs:49-75`). `resize_pane` cancels a divider drag and
  unzooms first (`:4811`, `:4839`).
- A layout change reaches each pane view through `sync_tab_layout`
  (`windows.rs:5508`), which pushes `TerminalGeometry` and calls
  `resize_to_layout` (`desktop.rs:2502`) when it changed. That reports a
  new grid only when the grid or cell size changed (`:2528-2546`). The
  renderer paints rows at the bounds' origin each frame
  (`renderer.rs:339-420`), and `GridGeometry` maps the pointer onto the
  displayed snapshot (`desktop.rs:2467`). The Ghostty engine rebuilds every
  row when the row or column count changed (`engine/ghostty.rs:600-605`).
- Pane commands are separate per direction. Navigation, resize, and zoom
  are unavailable on a tab with one pane (`windows.rs:7346-7365`). Default
  pane bindings live in `keymap.rs:751-822`. No default binding uses
  `cmd-ctrl-=` or `ctrl-super-=`.
- Menus: the macOS menu bar (`desktop.rs:164-221`) lists pane focus items
  and Move Pane to New Tab in its Window menu. The in-window window menu
  (`window_menu.rs:165-176`) and terminal context menu
  (`terminal_menu.rs:124-129`) list split and pane items. In-window menus
  have no submenus (`menu.rs:104`).
- `PanesConfig` (`crates/huterm-config/src/lib.rs:793`) holds only `zoom`
  and derives `Copy`; `huterm-config` already depends on
  `huterm-protocol`. The Settings window edits compound settings through
  `Special` editors, such as the named quake profiles
  (`settings_window/quake.rs`), and `SettingKind` has no list kind.

## Scope

In scope:

- A small template language for pane layouts, in which the five built-in
  layouts (equal columns, equal rows, main left, main top, and tiled) are
  defined.
- Arranging a tab into any named layout through one command with a
  `layout` argument.
- Next and previous layout cycling through a configurable list, with
  default bindings.
- Balancing the active tab's whole split tree.
- One runtime operation that rearranges one tab's existing panes, shared by
  all of the above.
- User-defined layouts in the config file and the Settings window (PR 3).
- Small-window fallbacks, focus and zoom rules, menus, default bindings,
  and undo entries through #256.

Non-goals:

- Saving the current tab's layout as a named layout ("Save Layout As…").
  It fits the template language later but needs its own design.
- A visual layout editor or a layout preview in Settings (settled decision 11).
- Balancing only the focused pane's split, as tmux's `select-layout -E`
  does.
- A layout that applies to every tab of a window.
- Animating the transition.

## Principles

The split panes and pane moves principles hold: one layout function,
structure shared and presentation local, moves conserve terminals, and one
event per operation. This plan adds:

- **Shape is pure, fitting is local.** Templates, their expansion, and
  balancing are pure functions in `huterm-protocol`, so the runtime, every
  client, the config validator, and tests share them. Only the initiating
  view knows pixels, so it fits the result to its minimum pane sizes, as
  it already does for splits and resizes.
- **Layouts keep traversal order.** No template reorders panes, so cycling
  never shuffles them. Swaps and moves remain the way to reorder.
- **Built-ins are templates.** The five built-in layouts are template
  strings in the language users write, so one implementation serves both.
- **Cycling is derived, not remembered.** Where a cycle continues from is
  read from the shared layout, so every window steps the same way and no
  new state exists.

## Decisions

### Pane order and the main pane

Layouts fill their slots in **traversal order**: depth first, children in
order, the order `focus_next_pane` and `move_pane_next` already use. For
`[(A over B) | C]` that order is A, B, C.

The **main pane is the first pane in traversal order** (settled decision
1). That is tmux's rule: `layout_set_main_v` takes the window's first tiled
pane as main ([layout-set.c][tmux-layout-set] on tmux `master` at
`523ac696`, read on 2026-10-27). A user makes another pane main by swapping
it first.

Template expansion hands panes to slots in the template's own traversal
order, so the result's traversal order always equals the input's. Applying
any sequence of layouts leaves the panes in the order they started in.

### Layout templates

A template describes a layout for any number of panes:

```text
template := split
split    := axis "(" child ("," child)* ")"
          | "grid" "(" "*" ")"
axis     := "columns" | "rows"
child    := node [share]
node     := split | "pane" | "*"
share    := integer "%"                        1 to 99
```

Whitespace may appear between tokens and is skipped there; it cannot
split a keyword or a number. `columns` places its children left to right, `rows`
top to bottom, and `grid(*)` tiles the panes its `*` receives (see
"Built-in layouts"). The root is a split or a grid. A split needs at least
two children unless one of them is a `*`. A template is at most 1 KiB and
16 levels deep, and a config holds at most 64 user layouts, which bounds
the cost of a hostile config file. A share too large to parse is an
error, never a panic.

**Slots.** `pane` takes one pane. `*` takes a share of the panes left once
every `pane` slot is counted. With `n` panes, `f` pane slots, and `k`
stars, the `r = n − f` remaining panes are divided as evenly as possible:
each star takes `floor(r / k)`, and the first `r mod k` stars, in template
order, take one more (settled decision 7). Slots then take their panes in
template order. For `columns(*, pane 20%, *)` and six panes, `r = 5`, so
the stars take 3 and 2: the first star takes p1 to p3, the `pane` takes
p4, and the second star takes p5 and p6.

**Fewer panes than slots.** When `n < f`, the first `n` pane slots, in
template order, take the panes; the rest stay empty and every star takes
none. A template with no `*` and more panes than pane slots has nowhere to
put them and is unavailable for that count (settled decision 10).

**Building the tree.** In a `columns` or `rows` split, a star with `m`
panes becomes `m` siblings in that split, sharing the star's share equally.
In a `grid`, they tile. Then, bottom up:

- an empty slot, and a split left with no children, drop out, and their
  siblings' shares scale up proportionally to fill the split;
- a split left with one child is replaced by that child, which takes the
  split's share;
- a child split on its parent's axis flattens into the parent, each of
  its children's fractions multiplied by its own. As in `normalize_child`
  (`pane_layout.rs:914`), a flatten that would pass `MAX_SPLIT_CHILDREN`
  is skipped and the nested split stays.

**Shares.** A child's `%` is its share of its parent split. Children
without one divide whatever the explicit shares leave, equally. Shares are
resolved from the template as written, before any slot drops: a star
counts as one child however many panes it receives, so in
`columns(pane, *)` the pane's fraction stays one half at any pane count.
Its weight is then within a unit of 500,000, because of the rounding
below: 500,000 with two or three panes, 499,999 with four. Explicit
shares must total less than 100 when some child has none, so each of those
children gets a positive share, and exactly 100 when every child has one.
A `*` or a split may carry a share like any child; a star's share is
divided among its panes.

**Weights.** Each child's fraction of its split stays unrounded through
expansion and flattening, as an `f64` computed in one fixed order, so every
client gets the same result. Each split of the final tree converts its
children's fractions to weights once: every child first gets 1 unit, and
the remaining `SPLIT_TOTAL − c` units, for `c` children, are apportioned
in proportion to the fractions by largest remainder, ties going to the
earlier child. The weights are therefore at least 1 and sum to exactly
`SPLIT_TOTAL`, as `validate` requires (`pane_layout.rs:650-656`), even for
a fraction far below one unit. Equal fractions give **equal weights**:
333,334, 333,333, and 333,333 for three children, as `SPLIT_TOTAL`
divided by the count with the remainder going one unit each to the first
children. Main left's 60 % and 40 % give exactly 600,000 and 400,000.

For `columns(*, pane 20%, *)` each star's share is 40 %. With six panes the
first star's three panes take 40 % / 3 each, the pane 20 %, and the second
star's two panes 20 % each, giving 133,334, 133,333, 133,333, 200,000,
200,000, and 200,000. With two panes the second star is empty, so the
first star's pane and the `pane` scale from 40 % and 20 % to two thirds
and one third: 666,666 and 333,334.

**Limits.** An expansion that leaves a split with more than
`MAX_SPLIT_CHILDREN` (256) children is unavailable for that pane count.

**Names.** A layout name is 1 to 32 characters of lowercase letters,
digits, and underscores, starting with a letter. The built-in names are
reserved.

### Built-in layouts

| Name | Title | Template |
| --- | --- | --- |
| `columns` | Equal Columns | `columns(*)` |
| `rows` | Equal Rows | `rows(*)` |
| `main_left` | Main Left | `columns(pane 60%, rows(*))` |
| `main_top` | Main Top | `rows(pane 60%, columns(*))` |
| `tiled` | Tiled | `grid(*)` |

Let `n` be the tab's pane count and `p1 … pn` its panes in traversal
order. Every layout of one pane is that pane alone, so `n = 1` is always a
no-op. The templates give these trees:

- **Equal columns:** one `Columns` split of `p1 … pn`, equal weights.
- **Equal rows:** one `Rows` split of `p1 … pn`, equal weights.
- **Main left:** for `n = 2`, `Columns[p1, p2]`; for `n ≥ 3`,
  `Columns[p1, Rows[p2 … pn]]` with the stack at equal weights. The main
  pane's weight is 600,000 (60 %, settled decision 2), and the other child
  takes 400,000.
- **Main top:** the same with the axes exchanged: `Rows[p1, Columns[p2 …
  pn]]`, main at 60 %.
- **Tiled:** `columns = ceil(sqrt(n))` and `rows = ceil(n / columns)`
  (settled decision 3). Panes fill rows in order, `columns` per row; the
  last row holds the rest. A `Rows` split of the rows at equal weights
  holds each row as a `Columns` split of its panes at equal weights, or as
  the pane itself when the row has one. With one row, tiled is the columns
  tree. The last row's panes share its full width equally, where tmux
  stretches the last pane.

| n | Tiled grid (panes per row) |
| --- | --- |
| 2 | 2 (same as columns) |
| 3 | 2, 1 |
| 4 | 2, 2 |
| 5 | 3, 2 |
| 6 | 3, 3 |
| 7 | 3, 3, 1 |
| 9 | 3, 3, 3 |
| 10 | 4, 4, 2 |

tmux grows rows first (`rows++` before `columns++` in `layout_set_tiled`),
so 2 panes stack and 5 panes make 3 rows of 2. Huterm grows columns first
because terminal windows are usually wider than tall and cells are about
twice as tall as wide.

Columns and rows of more than 256 panes, and a main stack of more than 256,
exceed `MAX_SPLIT_CHILDREN`, so those layouts are unavailable at those
counts: `arrange_panes` refuses them with a notice, and matching and
cycling skip them. Cycling 257 panes visits main left, main top, and tiled.
The minimum pane size refuses such counts long before this in any real
window.

### Protocol API

Beside the existing transforms in `huterm-protocol`:

```text
LayoutTemplate::parse(&str) -> Result<LayoutTemplate, TemplateError>
LayoutTemplate::expand(&self, panes: &[PaneId])
    -> Result<Arrangement, LayoutError>            unavailable: Err
NamedLayout { name, title, template }              BUILT_IN_LAYOUTS: 5
Arrangement = Pane(PaneId)
            | Split { axis, children: Vec<(weight, Arrangement)> }

PaneLayout::balanced(&self) -> Arrangement
PaneLayout::arranged(&self, &Arrangement, fresh: impl FnMut() -> SplitId)
    -> Result<PaneLayout, LayoutError>
PaneLayout::matching_layout(&self, cycle: &[NamedLayout]) -> Option<usize>
PaneLayout::step_layout(&self, cycle: &[NamedLayout], forward: bool,
    fit: impl FnMut(&NamedLayout, Arrangement) -> Option<Arrangement>)
    -> Option<(usize, Arrangement)>
```

`TemplateError` names the byte offset and the rule broken, so config
validation and the Settings window can show "expected `,` or `)` at 18".
`step_layout` holds the whole cycling rule (see "Cycling"), so a future
text client steps exactly as the desktop does and supplies only its own
`fit`.

An `Arrangement` is a split tree of pane IDs without split IDs or
terminal data. `arranged` builds the layout: each pane ID must name a pane
of `self` exactly once, and every pane of `self` must appear, so no pane or
terminal can appear or vanish. It takes each `PaneInfo` from `self`, so a
caller cannot change a pane's terminal or fallback name. When the
arrangement has the same shape as `self`, `arranged` keeps `self`'s split
IDs in place and only the weights change. Otherwise every split takes a
fresh ID from `fresh`, in traversal order. The result validates as every
layout does.

### Balancing

Balancing keeps the tree and gives every split's children equal weights,
recursively, down the whole tab. Topology, split IDs, and pane order are
unchanged. This is Ghostty's `equalize_splits`, whose weight counts leaves
through same-direction splits and counts a perpendicular split as one
([split_tree.zig][ghostty-split-tree] on Ghostty `main` at `919dca5c`, read
on 2026-10-27). Huterm flattens same-axis splits, so equal sibling weights
are the same rule.

The result depends only on the tree, never on the current weights, so
balancing twice gives the same layout, and the second commit is a no-op.
Fitting to the minimum sizes (below) depends only on the tree and the
view's size, so it keeps that property within one view.

### Fitting to the minimum pane size

The runtime knows weights, not pixels. As for splits and resizes, the
initiating view fits the ideal arrangement before it commits. A pure
function in `pane_geometry` does this:

```text
fit(arrangement, content bounds, scale, cell size, padding) -> Fit
Fit = Fits(Arrangement) | Short(Arrangement)
```

It works top down. For each split it measures the space along the split's
axis, after dividers, and each child's minimum extent along that axis:

- for a pane, the smallest frame whose `TerminalLayout` grid holds the
  minimum cells on that axis, with the title bar and padding, exactly, with
  no safety margin;
- for a child split on the same axis (only at the child limit), the sum of
  its children's minimums and its dividers;
- for a child split across the axis, the largest of its children's
  minimums.

When the minimums fit, children whose ideal share is below their minimum
get their minimum, and the others divide the rest in proportion to their
ideal weights, repeated until no child is short. Each round fixes at least
one more child, so it ends within the child count. Pixels convert to
weights by largest remainder, each at least 1 and summing to
`SPLIT_TOTAL`. The function then descends into each child with the bounds
`layout_panes` would give it.

The oracle is the real geometry: the result's `pane_grids` must pass
`fits_minimum`, the check a split preflight uses, so a layout the existing
preflight accepts is never `Short`. When boundary snapping leaves a pane
one physical pixel under its minimum, the fit moves that boundary one
physical pixel toward the sibling with the most room above its minimum and
checks again, at most once per boundary. A result that still fails is
`Short`.

When the minimums do not fit, the split's children get space in proportion
to their minimums and the result is `Short`. That matches the layout
function's rule for space another view imposes: every pane shrinks
together.

The placeholder layouts `fit` checks need distinct split IDs, because
`PaneLayout::new` rejects repeats. `fit` passes `arranged` a counter of
probe IDs in a runtime no Mux uses, extending the single `probe_split`
(`pane_moves.rs:173`).

In a window large enough for every minimum, which is the usual case, the
fitted arrangement equals the ideal one, so layouts and balancing give the
same weights in every window.

### Small windows

- **`arrange_panes`** with a layout that is `Short` in this view is refused
  with a notice, "Window too small for the main left layout", and changes
  nothing, as a split that would leave a pane under the minimum is refused.
- **Cycling** skips layouts that are `Short` and takes the next one that
  fits. With none, it shows a notice (see "Cycling").
- **An unavailable layout,** one that cannot hold this many panes, is
  refused by `arrange_panes` with a notice naming the limit: "The pair
  layout holds at most 2 panes" for a template without `*`, or "The
  columns layout holds at most 256 panes" at the split limit.
- **Balancing** never refuses (settled decision 6). When the minimums fit,
  it commits the fitted weights. When the window is already too small for
  the tab's tree, no weights can satisfy every minimum, and balancing
  commits the proportional fallback above. That is still idempotent in this
  view.

Other windows showing the tab apply the layout function's existing
shrinking rule; nothing is refused there.

### Cycling

The cycle is the `[panes] layout_cycle` list of layout names, by default
the five built-ins in table order: columns, rows, main left, main top, and
tiled. Users add their own layouts to it by name (see "User-defined
layouts"). Next moves forward through the list and previous moves back,
wrapping.

**Where a cycle starts.** `matching_layout` compares the current layout
with the ideal expansion of each available layout in the cycle, for the
same panes, ignoring split IDs. It uses ideal results, not fitted ones, so
every window finds the same match:

1. among layouts whose result has the same shape, ignoring weights, the one
   whose ideal weights are nearest the current weights: the smallest sum of
   absolute weight differences over the splits in traversal order;
2. ties, including an exact match shared by two layouts, go to the first in
   cycle order;
3. with no layout of the same shape, there is no match.

The built-ins' shapes differ except with two panes, where main left and
columns share one shape, main top and rows share another, and tiled equals
columns exactly. Nearest weights tell them apart: a main left layout
resized from 60/40 to 65/35 still matches main left, and a 55/45 layout,
equally near both, matches columns. A user layout that expands to the same
tree as another layout in the cycle is resolved the same way.

Matching reads only the tree, so a layout applied with `arrange_panes` but
missing from the cycle matches a cycle layout whose expansion has the same
shape, and otherwise nothing. With `layout_cycle = ["columns", "rows"]`,
main left on two panes is a 60/40 columns tree, matches columns, and next
applies rows; on three panes it matches nothing, and next starts the cycle
from its beginning. Next steps
forward from the match, and previous steps back. With no match, next starts
at the first layout in the cycle and previous at the last, as tmux starts
at its first or last layout when it has none recorded (`layout_set_next`
and `layout_set_previous`).

**Each step** (`step_layout`) tries each layout in the cycle at most once,
in cycle order, starting after the match (or at the first or last with no
match) and wrapping, so the matched layout itself comes last. It takes the
first that is available, that `fit` does not report `Short` in this view,
and whose fitted result differs from the current layout, weights included.
It compares the fitted result, which is what would be committed, so a step
never commits a no-op while another layout could change the tab. With
none, it does nothing and shows "No other pane layout fits this window"
when some candidate was `Short`, or "No other pane layout in the cycle
changes these panes" when every candidate was unavailable or equal to the
current layout, as with `layout_cycle = ["dev"]`.

With the default cycle, a full cycle from a matched layout visits every
available layout and returns to the starting layout's canonical form: five
distinct layouts for 3 to 256 panes; four for two panes, because tiled
equals columns (the step after main top applies tiled, which then matches
columns); and three for 257, where columns and rows are unavailable. From
258 panes the main stack would exceed `MAX_SPLIT_CHILDREN` too, so tiled is
the only available built-in: next or previous from an unmatched layout
applies it, and from tiled shows the notice. From an unmatched layout, the
first step enters the cycle and the rest follow it.

**After a manual resize** the shape still matches and the nearest weights
usually still pick the same layout, so the cycle continues from it.
Reapplying a named layout restores its ideal weights. **After a split,
close, move, or swap** the shape usually matches no layout, and the cycle
starts over. A split along the axis of a columns or rows layout keeps that
shape, so the cycle continues. Every window computes the same match from
the shared layout; only the `Short` skip depends on the view.

### Runtime operation

```text
Mux::arrange_panes(workspace, tab, expected: &PaneLayout,
                   arrangement: &Arrangement) -> Result<LayoutChange>
```

One operation serves named layouts, cycling, and balancing:

- It validates the workspace and tab as `resize_split` does, and refuses a
  tab whose layout no longer has `expected`'s structure: the same shape,
  split IDs, pane IDs, and terminal IDs, with weights ignored. That is the
  same predicate #256's restore guard uses (see "Undo"). A refusal returns
  `InvalidLayout(LayoutError::ChildrenChanged)`, the error a stale divider
  commit gets.
- It builds the result with `arranged`. An arrangement equal to the
  current layout, weights included, is a no-op: it emits nothing,
  allocates nothing, leaves the shared zoom as it is, and returns an empty
  `LayoutChange`. This covers one pane, reapplying the current layout, and
  balancing a balanced tab. The desktop never sends such an arrangement
  (see "Desktop flow"); the rule is for other clients and races.
- A result of a different shape needs one fresh split ID per split. It
  checks with `next_id.checked_add(count)` that they exist before any
  change, as the move preflight does, and refuses with `IdExhausted`
  otherwise.
- It clears the tab's shared zoom (see "Focus and zoom"), commits, emits
  one `PaneLayoutChanged { tab, layout, zoomed }`, and returns the
  `LayoutChange` of that tab, before and after. A weights-only balance
  clears the shared zoom too, as tmux's `select-layout` unzooms for any
  layout.
- It leaves `Mux::revision` alone. No terminal changes tabs, so no close
  ticket's scope changes, and a layout change in one window must not bounce
  another window's pending close confirmation, the reason `resize_split`
  and swaps leave it alone. The fresh split IDs therefore come from a new
  `reserve_ids(count)` that advances `next_id` without `changed()`; split
  IDs never appear in a close ticket. `allocate` keeps its behavior for
  every other caller, and the revision's doc comment gains arrangements.

It starts and stops no runtime, so it holds the Mux lock briefly, as
`resize_split` does. The reducer needs no change: `PaneLayoutChanged`
already validates the layout and its cross-tab uniqueness, and the digest
describes state, so it is unchanged. Terminal content generations do not
move: a layout change is a hierarchy event, not terminal output.

Why one operation that takes an arrangement, rather than a layout name in
the runtime: the minimum-size fit needs pixels, which only a client has, a
future text client will fit to its own cells, and user layouts live in the
client's config, not the runtime. The runtime still owns
validity: conservation, the structure guard, identity allocation, and the
invariants.

### Desktop flow

For `arrange_panes`, a cycle step, or balancing, the window:

1. requires the active tab to hold at least two panes, the window not to be
   busy, no close confirmation, and no arrangement of this window in
   flight;
2. reads the tab's committed layout (`committed_layout`, which includes
   this window's queued resizes);
3. resolves the layout, or each layout in the cycle, by name against the
   built-ins and the current config, then computes the ideal arrangement
   and fits it in this view's tab content bounds, unzoomed, with title
   bars and dividers, as a split preflight does. An unknown name, such as
   a binding's layout removed from the config, shows "No pane layout named
   `dev`" and changes nothing;
4. refuses or skips a `Short` or unavailable layout as above;
5. does nothing more when the fitted arrangement equals the committed
   layout: no commit, no unzoom, no notice, and no undo entry, so a second
   balance is a true no-op;
6. cancels this window's divider drag on the tab and clears this view's
   own zoom of the tab locally, then calls `sync_zoom`, as a move does
   (`pane_moves.rs:441-464`). It sends no `set_pane_zoom`: in shared mode
   `arrange_panes` clears the shared zoom in its own event, so the
   operation stays one event and the recorded `before` keeps the zoom for
   undo. The window remembers the zoom it cleared: when the runtime refuses
   the arrangement, or returns an empty change because another window's
   resize made it a no-op (step 8), it zooms that pane again, unless the
   pane left the tab or the view zoomed another pane meanwhile;
7. queues the commit on the window's ordered commit queue, after any resize
   commits already queued, so a balance pressed after a resize lands after
   it. The queue becomes a layout commit queue carrying either a resize or
   an arrangement;
8. on completion, once the projection reaches the commit's sequence,
   records a non-empty change as a #256 undo entry, or shows a notice for a
   refusal: "The panes changed; try again" for a structure change, or the
   error for anything else.

An arrangement does not hold `busy`. `busy` drops terminal keystrokes
(`input_blocked`, `windows.rs:1823-1841`) and makes reconcile defer
adoptions and removals until the holder's completion reconciles
(`projection.rs:727-756`), which a move needs because it changes
membership. An arrangement changes no membership, so reconcile has nothing
to defer, and typing right after the balance key reaches the shell.
Instead, a per-window count of arrangements in flight makes
`check_available` (`windows.rs:7504-7518`) refuse, as `busy` does, and
refuses a new divider press. Everything `busy` gates is unavailable until
the completion: every structural command, including resize, split, move,
swap, zoom, undo, and redo, and also focus navigation, tab switching, new
tabs, the palette, and menus, so none computes from a layout the arrangement is
about to replace. Without that, a resize command pressed right after
Balance would build its `ResizePreview` from the pre-balance weights
(`resize_pane`, `windows.rs:4851-4862`) and write them back behind the
balance. Terminal input (`input_blocked`, `windows.rs:1823`) and reconcile
ignore the count. The commit takes one Mux lock in process, so the window
is rarely unavailable for longer than a frame.

Routes that do not pass `check_available`, such as a pane drag that
started before the arrangement was queued, can still commit and complete
out of order with it. A completion can resume after the projection already
holds a later commit, because waiting for an applied sequence returns at
once (`wait_for`, `projection.rs:257-265`). History therefore records each
entry with its commit's hierarchy sequence and ignores a completion whose
sequence is older than its newest entry's, so a late arrangement completion
never replaces the undo entry of a swap that committed after it.

There is no preview. The layout changes on the next projection, as it does
after a move command. Each pane whose size changes shows the resize
indicator, as after a divider drag.

### Focus and zoom

- **Focus** is view-local and stays on the same pane in every window:
  panes keep their IDs, and each `TabView` keeps its focused pane and
  focus history. Reconcile keeps every pane view, and the pane order sort
  from #29 finds nothing to reorder, because layouts keep traversal order.
- **The initiating view** unzooms when it commits a change, because a
  layout change or balance is invisible behind a zoom. tmux's `select-layout`
  also unzooms. In view mode it clears its own zoom locally; in shared mode
  the runtime clears it (step 6 above). Focus stays on the pane that was
  zoomed. A no-op leaves the zoom alone, and an arrangement the runtime
  refuses or finds empty gives the view its zoom back.
- **Shared zoom** (`[panes] zoom = "shared"`): `arrange_panes` clears the
  tab's shared zoom, as `split_pane` and within-tab moves do, so every
  window in shared mode shows the new layout. The cleared zoom travels in
  the same `PaneLayoutChanged`, and no separate `PaneZoomChanged` is
  emitted.
- **Other windows in view mode** keep their own zoom. Its pane is still in
  the tab, so it is still valid and stays on the same pane. Their divider
  drags follow the existing rule: a drag whose split vanished or whose
  children changed cancels, so a layout of another shape cancels it, and a
  balance lets it continue from the new weights.

### Repainting and reflow

`PaneLayoutChanged` marks the tab in `Touched::layouts`, and reconcile
notifies every window showing the tab (#28 PR 4 deviation). Then, per pane
view, through the existing path:

- `sync_tab_layout` pushes the new `TerminalGeometry` and notifies each
  view whose geometry or placement changed, even when its grid is
  unchanged. Dividers and title bars repaint from the same `TabPanes`
  output, and hit tests use the last drawn frame.
- A pane whose grid is unchanged, because it only moved or changed size by
  less than a cell, requests nothing. It repaints its retained rows at the
  new origin, because the renderer places rows from the bounds each frame
  and `GridGeometry` maps the pointer from the same origin. It advances no
  content generation and requests no snapshot.
- A pane whose grid changed reports it through its viewer. Until the
  runtime's snapshot at the new geometry revision arrives, the view clips
  the old snapshot to its new bounds, and `GridGeometry` maps the pointer
  and selection onto the displayed grid. It never repositions cached rows
  for the new bounds. A controlling view's PTY resizes and reflows, and the
  engine extracts every row again because the row or column count changed
  (`ghostty.rs:600-605`). That extraction may still share an immutable row
  value it found identical, but it never trusts a row's position or content
  from before the change. Nothing in this work may skip that full
  extraction or reuse rows by their old positions.
- A layout change can move every pane at once, including panes another window
  shows in an inactive tab. Hidden views resize through the layout sync,
  as they do for a window resize.

Layout invalidation never compares content generations, as the desktop
client guide requires.

### Commands, bindings, and menus

One command arranges panes into any named layout, so built-in and user
layouts work the same way in the palette, in bindings, and in menus
(settled decision 5):

| Command | Palette title | Arguments | macOS | Linux |
| --- | --- | --- | --- | --- |
| `arrange_panes` | Arrange Panes… | `layout` (Layout, prompted) | none | none |
| `next_pane_layout` | Next Pane Layout | none | `cmd-ctrl-]` | `ctrl-super-alt-]` |
| `previous_pane_layout` | Previous Pane Layout | none | `cmd-ctrl-[` | `ctrl-super-alt-[` |
| `balance_panes` | Balance Panes | none | `cmd-ctrl-=` | `ctrl-super-=` |

All are Window scope, and unavailable on a tab of one pane, while busy,
during a close confirmation, or while this window has an arrangement in
flight.

`ArgumentKind::Layout` joins the catalog, modelled on
`ArgumentKind::QuakeProfile` (`command.rs:64-66`), which is also a name
the config defines: carried as `CommandValue::Text`, settable in a
binding, given a plain string schema, resolved when the command runs, and
prompted in the palette with rows from the config. The catalog validates
only the name's syntax, and the window resolves the name at run time (step
3 of "Desktop flow"). PR 2's schema describes the argument as one of the
built-in names; PR 3 extends the description to `[panes.layouts]` entries.
The picker lists the built-ins by title, then, from PR 3, user layouts by
name, each with its template as detail text.

The Settings keybinding editor offers only argument kinds its
`configurable()` filter lists (`settings_window/keybindings.rs:329-340`),
and that match is not exhaustive, so a new kind would compile and be
silently hidden. PR 2 adds `Layout` there with a dropdown of the
built-ins, as `QuakeProfile` has one of profiles, and PR 3 adds the user
layouts to it. A binding names a layout directly:

```toml
[[keybinding]]
key = "cmd-alt-1"
command = "arrange_panes"
args = { layout = "dev" }
```

Default bindings (settled decision 4):

- **Cycling,** as tmux binds `next-layout` to `prefix Space`:
  `cmd-ctrl-]` and `cmd-ctrl-[` on macOS, the layout counterparts of the
  `cmd-]` and `cmd-[` pane focus keys, and `ctrl-super-alt-]` and
  `ctrl-super-alt-[` on Linux, beside the `ctrl-super-]` focus keys. The
  shifted Linux form would fold to `}` in GPUI. Space-based keys collide:
  `cmd-ctrl-space` opens the macOS character viewer, `cmd-alt-space` opens
  a Finder search window, and GNOME switches input sources on
  `super-space`.
- **Balance:** `cmd-ctrl-=` is Ghostty's macOS `equalize_splits` default
  ([Config.zig][ghostty-config] at `919dca5c`); Ghostty binds nothing on
  Linux. `ctrl-super-=` follows the `ctrl-super` family of Huterm's Linux
  pane keys. KWin binds `meta-=` to zoom in, which the extra `ctrl`
  avoids.

None of the six keys is a Huterm default today (`keymap.rs:751-822`), and
none collides with #256's `cmd-z`, `cmd-shift-z`, `ctrl-shift-z`, or
`ctrl-shift-y`, with `cmd-ctrl-<arrow>` resize, or with `cmd-shift-enter`
zoom. PR 2 checks the Linux keys against GNOME and KDE defaults and lists
any conflict in `docs/keybindings.md` with the existing ones.
`arrange_panes` stays unbound: users bind it per layout, as above.

Menus:

- macOS Window menu: Arrange Panes…, Next Pane Layout, Previous Pane
  Layout, and Balance Panes, after Move Pane to New Tab. Arrange Panes…
  opens the palette on the command's layout picker.
- In-window window menu (Linux, and macOS without the menu bar): Arrange
  Panes… and Balance Panes, after Close Pane.
- Terminal context menu in split tabs: Balance Panes, after Close Pane.

Notices name a built-in by its title in prose and a user layout by its
name: "Window too small for the main left layout", "Window too small for
the dev layout".

### User-defined layouts

PR 3 lets users name their own layouts in the template language and choose
the cycle (settled decision 8):

```toml
[panes.layouts]
dev = "columns(pane 55%, rows(pane 70%, columns(*)))"
pair = "rows(columns(pane, pane) 70%, columns(*))"
sides = "columns(rows(*) 25%, pane, rows(*) 25%)"

[panes]
layout_cycle = ["columns", "rows", "main_left", "main_top", "tiled", "dev"]
```

`sides` shows two stars sharing the panes around a centre pane: with six
panes, the left stack takes three, the centre pane one, and the right
stack two. With one pane it is that pane alone. With two, the right stack
is empty, so the left pane's 25 % and the centre's 50 % scale to one third
and two thirds: weights 333,334 and 666,666.

- `PanesConfig` gains `layouts`, a map from name to template string, and
  `layout_cycle`, a list of names that defaults to the five built-ins
  (settled decision 9). `PanesConfig` stops being `Copy`.
- Config validation parses every template with the protocol's
  `LayoutTemplate::parse`, which `huterm-config` can call because it
  already depends on `huterm-protocol`. It rejects a bad template, an
  invalid or reserved name, and a `layout_cycle` that is empty, repeats a
  name, or names an unknown layout. Each error carries its key path and
  the parser's offset, through the same path as every other config error.
- The published schema describes both keys, with the grammar summarized in
  the description and the built-in names listed. PR 3 regenerates it.
- `docs/configuration.md` documents the language with examples,
  including `columns(pane, *)`, whose pane keeps about half the width, and
  notes
  that a `layout_cycle` entry naming a removed layout makes the whole
  config invalid, as a global shortcut naming a missing quake profile
  does, while a keybinding naming one fails only when it runs.
  `docs/keybindings.md` shows bindings with a `layout` argument.
- A config reload replaces the layouts and the cycle. Nothing is cached
  per tab, so the next command resolves names against the new config, and
  a binding whose layout was removed shows the unknown-layout notice.

### Settings window

PR 3 adds a Pane Layouts group to the Panes settings. Adding and removing
layouts follows the named quake profiles editor (`Special::QuakeProfiles`,
`create_profile` and `delete_profile` in `settings_window/quake.rs:473`,
`:523`); renaming is new, because profiles cannot be renamed.

- `Special::PaneLayouts` edits `panes.layouts`. Built-ins show as
  read-only rows with their templates. Each user layout is a row with a
  name field and a template field, and the group has add, rename, and
  remove actions.
- Name and template fields validate on every keystroke, with the parser's
  message below the field, such as "expected `,` or `)` at column 18"
  (offsets index the text as typed, shown as a one-based character
  column). They save only on commit, Enter or leaving the field, as other
  Settings text fields do (`settings_window.rs:996`), so a template
  passing through `5%` on its way to `55%` writes nothing. An invalid
  template or name is never saved.
- `Special::LayoutCycle` edits `panes.layout_cycle`, because Settings has
  no list control (`SettingKind` has no list kind). It shows every layout,
  built-in and user, as an ordered row with an include checkbox and
  move up and move down buttons. Unchecking the last included layout is
  refused, since the cycle cannot be empty.
- A rename is three patches in one save: remove the old key, set the new
  one, and rewrite `layout_cycle` when it names the layout. Removal is two:
  the key and the cycle entry. The settings queue carries one patch per
  save today (`saving.rs:651`), so PR 3 lets it carry a batch;
  `ConfigFile::apply` already takes one (`crates/huterm-config/src/
  document.rs:386-398`) and checks every path for conflicts, so a failed
  save changes nothing. `Patch` has no rename, and a `Set` on a table
  merges keys rather than removing one (`document.rs:120-125`), so a
  renamed entry moves to the end of `[panes.layouts]` and loses comments
  attached to the old key.
- Removing a layout that is the cycle's only entry is refused with "Add
  another layout to the cycle first", as unchecking the last row is.
- Renaming or removing a layout that a keybinding names is refused with a
  message naming the binding, as `delete_profile` refuses to delete a
  quake profile a global shortcut uses (`quake.rs:539-551`) (settled decision
  12). Bindings in the file are never rewritten.
- Saving goes through the existing settings write path, so comments and
  formatting elsewhere in the file survive, as for every other setting.

### Undo (#256)

Assuming #256 lands first with single-entry stacks:

- `arrange_panes` returns the `LayoutChange` #256 records, with `placed`
  empty, because it places no tab.
- The window's completion (step 8 above) pushes one `HistoryEntry`: a
  `label` naming the layout, such as "Arrange Panes: Main Left" or
  "Arrange Panes: dev", for `arrange_panes` and cycle steps, and "Balance
  Panes" for balance; the initiating view's focused pane as `focus`; and
  the change. A new entry clears the redo stack, as for moves.
  A no-op records nothing.
- Undo runs `restore_tabs` on that one tab. Its guard compares shape, split
  IDs, pane IDs, and terminal IDs and ignores weights, so:
  - a layout's undo restores the old tree and its old split IDs, which are
    at or below `next_id` and in no other tab, because the layout retired
    them;
  - a balance's before and after share a shape, so its entry stays
    undoable across later divider drags, and undo restores the weights from
    before the balance, discarding those drags, which is #256's rule for
    every entry;
  - a split, close, move, or another window's layout in that tab changes
    the shape and prunes the entry.
- The restore emits `PanesRearranged` with one `Layout` change and leaves
  the revision alone, because no terminal changes tabs.
- A layout change cleared the shared zoom in its own event, so the recorded
  `before` still names the zoomed pane, and #256's zoom rule restores it
  when the current zoom is still none. View-local zoom is not recorded.
- `undo` and `redo` are unavailable while the window has an arrangement in
  flight, through the same `check_available` refusal as `busy`, so an undo
  cannot overtake the entry it should undo.

What #256 needs to expose, which the orchestrator should pass on now:

1. **A recording entry point any layout completion can call, ordered by
   commit.** #256's plan records in the shared move completion. #30 commits
   on the layout commit queue instead, so history needs a window method
   such as `record_layout_change(label, focus, change, seq)` that both
   call. It keeps the hierarchy sequence of its newest entry and ignores a
   completion with an older one, because a move's and an arrangement's
   completions can finish out of commit order (see "Desktop flow"). After
   it pushes, it prunes against the projection, as #256's completion does
   (`layout-undo.md:257-260`), because without `busy` another batch can
   land between the arrangement's event and its completion.
2. **The structure guard as a `PaneLayout` method**, for example
   `same_structure(&self, other) -> bool` (shape, split IDs, pane IDs, and
   terminal IDs; weights ignored), so `restore_tabs` and `arrange_panes`
   share one predicate.
3. **A weights-only change is a real entry.** A balance's `before` and
   `after` differ only in weights. `restore_tabs` must apply it and must not
   treat it as a no-op because the shapes match.

None of these changes #256's design. If #30 lands first instead, it ships
without undo, and #256's client PR records `arrange_panes` with the same
three points.

## Races

| Race | Handling | Evidence |
| --- | --- | --- |
| Another window splits, closes, moves, or rearranges the tab between this window's fit and commit | The structure guard refuses; nothing changes; one notice | Core; tier 2 (structure guard refusal) |
| Another window resizes a divider in the tab between fit and commit | The guard ignores weights; the arrangement applies, last writer wins as for divider commits | Core (weights-only change passes the guard) |
| This window's resize commits are queued when a balance is pressed | The layout commit queue applies them in order | Tier 2 |
| Another window shows a close confirmation for the tab | The revision is unchanged, so its commit stays valid | Core lifecycle test |
| Another window drags a divider of the tab | A layout of another shape cancels the drag; a balance lets it continue from the new weights | Tier 2 |
| Another window zooms a pane in view mode | Its zoom stays on the same pane | Tier 2 (convergence) |
| A pane exits while the commit is queued | With `close_on_exit = false` the pane stays and the guard passes; an exit-close changes the shape, the guard refuses, and the close proceeds, because no `busy` hold defers it | Tier 2 |
| A structural command (undo, a cycle step, resize, split) or a divider press in this window while an arrangement is in flight | Unavailable or refused until the completion; keystrokes still reach the shell | Tier 2 |
| This window's divider drag is in progress when a layout command runs | Step 6 cancels it before the commit is queued | Tier 2 |
| A pane drag that started earlier drops while an arrangement is in flight, and the two complete out of commit order | History keeps the entry with the newer commit sequence | Tier 2 (completion order) |
| The layout event and the tab's terminal output interleave | Terminal events route by pane, and the geometry rules above order the repaint | Existing rule |

## Testing and verification

Mapped to #30's acceptance criteria. Apply the global testing rules: each
new test fails at its assertion before its fix, or under one targeted
perturbation for a test over existing behavior, and the runner output names
it. Layout comparisons use #29's canonical form: panes by index in the
fixture's traversal order, split IDs erased, axes, child order, and weights
kept. Tier 2 tests control arrangements with two new stages in
`worker_hooks::Stage` (`worker_hooks.rs:11-19`): `Layout`, entered at the
start of each layout queue job before its runtime call, and
`LayoutCommitted`, entered after the runtime call returns and before the
completion waits for the projection and records history. Tests order
other changes against those gates, never with sleeps.

### Layouts, templates, cycling, balance, and minimum sizes (criterion 2)

- Protocol unit tests (PR 1):
  - each built-in for `n = 1` through 10 and for 257 and 258 panes, with
    exact trees and weights, including the tiled table and the 60 % main
    share; at 257 panes, columns and rows are unavailable, and at 258 the
    main layouts are too;
  - template parsing: every grammar form, whitespace, and each error with
    its offset: bad syntax, a share outside 1 to 99, explicit shares of
    100 or more beside a child without one, explicit shares not totalling
    100 when every child has one, `grid` around anything but
    `*`, a root that is not a split or grid, a one-child split without a
    `*`, whitespace inside a keyword, a share too large for its integer
    type, and the length and depth limits; offsets index the original
    text;
  - template expansion, each asserting the full canonical tree with
    weights: `columns(*, pane 20%, *)` for `n = 1` through 6, including
    the weights given under "Weights"; `sides` from "User-defined layouts"
    for `n = 1`, 2, and 6; `columns(pane, *)` with weights 500,000 and
    500,000 for two panes, 500,000, 250,000, and 250,000 for three, and
    499,999 and three of 166,667 for four; a template
    without `*` below, at, and above its slot count, unavailable above;
    dropped slots scaling their siblings' shares; a same-axis child
    flattening with its proportions; `columns(pane, columns(*))` with 257
    panes, whose flatten is skipped at the limit, leaving a valid nested
    layout; a `grid` nested in a split; and an expansion over 256
    children, unavailable;
  - a sweep of every built-in and test template for `n = 1` through 12
    through `expand` and `arranged`: each count where the template is
    unavailable, such as `columns(pane, pane)` above two panes, must
    return the unavailable error, and every other count must give a valid
    layout with unchanged traversal order. This catches mistakes in the
    order of the drop, collapse, and flatten rules;
  - weights: three equal children give 333,334, 333,333, and 333,333; main
    left gives 600,000 and 400,000; and the nested template
    `columns(columns(columns(columns(pane 1%, pane) 1%, pane) 1%, pane) 1%,
    pane)`, whose fractions are 0.00000001 to 0.99, gives exactly 1, 2,
    100, 9,901, and 989,996, every weight positive and the sum exact;
  - a nested, uneven input (`[(A over (B | C)) | D]` with uneven weights)
    gives the same trees as a flat input with the same traversal order;
  - every built-in and every test template keeps traversal order;
  - `balanced` on a nested tree gives equal weights at every split and
    keeps split IDs and shape, and `balanced` of `balanced` is equal;
  - `arranged` refuses a missing, repeated, or unknown pane, keeps split
    IDs for a same-shape arrangement, and takes fresh IDs in traversal
    order otherwise;
  - `matching_layout`: an exact match; a shape match after a weight change;
    two panes at 60/40, 65/35, 55/45, and 50/50, which match main left,
    main left, columns, and columns; no match; and 257 panes, where columns
    and rows are skipped;
  - `step_layout` with an identity `fit` over the default cycle, from each
    built-in and from an
    unmatched layout, for `n = 2` through 6 and 257: the expected sequence
    in both directions, never the current layout, and per full cycle from
    a match five distinct layouts for `n = 3` through 6, four for `n = 2`,
    and three (main left, main top, tiled) for 257; at 258, a step from an
    unmatched layout gives tiled and a step from tiled gives nothing;
  - `step_layout` with a `fit` that reports one layout `Short`, and one
    whose fitted result equals the current layout while a later layout
    differs: both are skipped, and the step takes the later layout; with
    every candidate skipped, it returns nothing;
  - `step_layout` over a custom cycle that includes a user template and
    omits a built-in: the cycle follows the list; with `["columns",
    "rows"]`, main left on two panes matches columns and steps to rows,
    and main left on three panes matches nothing and steps to columns;
  - a cycle holding a user layout that expands to the same tree as a
    built-in: the match goes to whichever comes first in the cycle, and a
    step skips the other, because its result equals the current layout;
  - `layout_cycle = ["dev"]` with the tab already in `dev`: the step
    returns nothing, for the "no other layout changes these panes"
    notice;
- `pane_geometry` unit tests (PR 2), with real cell metrics and padding,
  asserting through `pane_grids` and `fits_minimum`, never through the
  fitter's own minimum:
  - `fit` equals the ideal arrangement in a large window;
  - a main share that would leave the stack under its minimum shrinks main
    just enough;
  - the exact boundary: the smallest content bounds where the existing
    split preflight accepts two equal columns also `Fits`, and one physical
    pixel less is `Short`, at scale factors 1 and 2, with zero and nonzero
    padding;
  - a tree that cannot fit is `Short`, with weights proportional to the
    minimums;
  - balance fitted twice at one size gives identical weights.
- Core tests (PR 1):
  - `arrange_panes` applies each built-in and a balance, and the differential
    reducer test checks the projection after each;
  - a no-op, a one-pane tab, a refused guard, a conservation failure, and
    exhausted identities each emit nothing and keep the layout, shared
    zoom, sequence, and revision;
  - a same-shape arrangement keeps split IDs and allocates nothing;
  - another writer's weights-only change between `expected` and the commit
    passes the guard, and the arrangement applies;
  - the shared zoom clears with the change, in the same and only event;
  - foreign workspace or tab IDs fail with `ForeignRuntime`.
- Core lifecycle test (PR 1): an arrangement between a close assessment and
  its commit leaves the commit valid.
- Tier 2 (PR 2):
  - `arrange_panes` with each built-in, next, previous, and balance on a
    three-pane nested tab gives the expected canonical layout, and the
    palette's layout picker and an explicit `layout` argument give equal
    layouts;
  - `arrange_panes` with an unknown layout name shows one notice and
    changes nothing;
  - a layout refused for a short window shows one notice and leaves the
    hierarchy sequence unchanged;
  - cycling skips a short layout;
  - balance pressed after queued resize commands lands after them;
  - balance, then, after its completion, balance again: the second press
    is accepted, sends no commit, records no undo entry, and leaves the
    hierarchy sequence unchanged;
  - the structure guard: another window's split forced between this
    window's fit and commit refuses the arrangement with one notice and
    changes nothing;
  - with `close_on_exit = true`, a pane exits while an arrangement is
    queued, and its exit-close is forced to commit first; the arrangement
    is refused with one notice and the pane's close completes;
  - while an arrangement is held at the `Layout` stage, `undo`,
    `next_pane_layout`, `resize_pane_right`, and `split_right` are
    unavailable, a divider press starts no drag, and a typed key reaches
    the shell;
  - a divider drag in progress when Balance runs is cancelled first;
  - completion order, with #256: the arrangement is held at
    `LayoutCommitted`, after its commit; a pane drag that started earlier
    drops and completes, recording its entry; the arrangement is then
    released, and the undo entry is still the drop's.

### Viewers converge, focus and zoom stay (criterion 3)

- Tier 2 (PR 2), two windows on one session showing one tab:
  - window A applies a layout; window B's projection holds the same
    layout, B keeps every pane view entity, its focused pane, its focus
    history, and its view-local zoom;
  - window A was zoomed in view mode: it unzooms and keeps focus on that
    pane;
  - shared mode: one `PaneLayoutChanged` and no `PaneZoomChanged` reach
    the projection, the shared zoom clears in both windows, and, with #256,
    undo restores the shared zoom;
  - window B's in-progress divider drag cancels under a layout and
    continues under a balance;
  - #256 recording (when it has landed): undo of a layout and of a balance
    restores the canonical layout, split IDs, and weights; a later split
    prunes the entry.

### PTY survival and sizes (criteria 1 and 3)

- Core PTY test (PR 1): a three-pane tab of real shells cycles through every
  built-in and a balance. After each, every shell answers a unique ACK, `$$`
  is unchanged, `kill(pid, 0)` succeeds, pane and terminal IDs are
  unchanged, and a line printed before the first change is still in the
  snapshot. Synchronize on ACKs, never sleeps.

### Repaint and reflow without stale rows (criterion 1)

- Tier 1 (`view_tests.rs`, PR 2): a geometry push that moves a view's
  origin without changing its grid repaints the same `Arc<TerminalRow>`
  values at the new origin, requests no snapshot, and maps a pointer
  position to the cell at the new origin.
- Tier 2 (PR 2), with real shells: one shell prints a line longer than its
  pane's narrower width after a layout change, and others stay idle. A
  layout then leaves one pane's grid unchanged and shrinks the printing pane's
  width:
  - the unchanged pane requests no resize and keeps its rows;
  - the shrunk pane reports one new grid and paints the old snapshot
    clipped until the new geometry revision arrives;
  - once it arrives, the displayed snapshot has the new size and shows the
    line wrapped at the new width, read from the snapshot's rows, so a
    client that kept old row positions fails;
  - a second layout that widens the pane again shows the line unwrapped.

  The existing clipping test,
  `a_larger_canonical_grid_is_clipped_and_owned_gestures_survive_it`,
  covers the clipping rule itself, and the engine's existing resize tests
  cover full extraction after a size change.

### Native alignment of painting and mouse coordinates (criterion 4)

- `check-overlays.ts` (PR 2), which both palette smokes run with AppKit or
  XTest input: split into three columns, apply main left and then tiled
  through catalog commands, and after each wait for smoke state to report
  the new layout. Then:
  - each pane's `stty size` matches its entry in `w<i>.pane_grids=`;
  - for a pane whose origin moves while its grid stays the same: arrange
    three balanced columns and record their grids; move both dividers
    right by the same number of cells with `resize_pane_right` on the left
    pane, then on the middle pane; record the middle pane's entry in
    `w<i>.pane_rects=`; balance; then require that the middle pane's grid
    in `w<i>.pane_grids=` is unchanged and its rect moved left, failing
    loudly if the grid changed. With application mouse tracking on in that
    pane, a native click at a known cell reports that cell's coordinates;
  - a native click in another pane focuses it and its shell ACKs a probe;
  - the `balance_panes` and `next_pane_layout` default keys reach no
    shell, checked with the ordered ACK probe, and the cycle key changes
    the layout;
  - after a native window resize, each pane's `stty size` and the clicked
    cell's reported coordinates match again.
- `smoke:macos-menus` (PR 2): the Window menu's Balance Panes shows
  `⌃⌘=`.
- Run the host macOS smokes and `mise run linux:smoke` in Docker, checking
  `df -h` first and cleaning volumes and images after. Never run
  `vm:macos:smoke`.

### User-defined layouts and Settings (PR 3)

- Config tests: `[panes.layouts]` and `layout_cycle` load; each rejection
  (a bad template with its offset, an invalid or reserved name, more than
  64 layouts, an empty
  cycle, a repeated name, an unknown name in the cycle) names its key
  path; a reload that removes a layout replaces the registry.
- `mise run schema:check` passes after `schema:generate`, and
  `smoke:schema-editor` shows completion for the two new keys.
- Keymap tests: a binding with `args = { layout = "dev" }` compiles, and a
  name with invalid syntax is a keymap error.
- Settings keybinding editor, as `window_tests/settings_keys.rs:604` tests
  profile arguments: an `arrange_panes` binding offers the `layout`
  argument with a dropdown of the built-ins (PR 2) and user layouts
  (PR 3), and writes the chosen name.
- Palette tests: the layout picker lists the built-ins by title, then user
  layouts by name, with templates as detail text.
- Tier 2:
  - a user layout from config, applied by a binding with a `layout`
    argument, gives the expected canonical layout;
  - a cycle that includes the user layout and omits a built-in steps in
    list order;
  - after a reload removes the layout, its binding shows one notice and
    changes nothing.
- Settings tier 2, through the existing settings window tests:
  - adding a layout with a valid template writes `[panes.layouts]`, and
    the new layout is usable at once;
  - an invalid template shows the parser's message and writes nothing;
  - renaming a layout renames it in `layout_cycle` in the same save, and
    removing it removes it from the cycle;
  - a rename whose save fails, for example because the file changed on
    disk, leaves both `[panes.layouts]` and `layout_cycle` unchanged;
  - renaming or removing a layout that a keybinding names is refused with
    the binding named, and writes nothing;
  - typing a template writes nothing until Enter or blur;
  - reordering and unchecking cycle rows writes `layout_cycle` in the new
    order; unchecking the last included row, and removing a layout that is
    the cycle's only entry, are refused with the message and write
    nothing;
  - the new rows and buttons are reachable by Tab and Shift-Tab, and focus
    moves to a neighbouring row when a row is removed;
  - comments elsewhere in the file survive a save.

### Commands

`mise run check` while iterating; focused `cargo test -p huterm-protocol`,
`cargo test -p huterm-core mux::`, and `cargo test -p huterm-gpui --features
test-support`; `mise run test:ghostty-releasesafe` for PR 1, which adds a
core PTY test; `mise run verify` before each PR leaves draft.

## Delivery

Three PRs, each leaving `main` consistent and shippable:

1. **PR 1, runtime arrangements.** In the protocol: the template parser and
   expansion, the five built-in layouts as templates, `Arrangement`,
   `balanced`, `arranged`, `matching_layout`, and `step_layout`. In core:
   `Mux::arrange_panes`, `reserve_ids`, and the revision doc. The
   protocol, core, lifecycle, and PTY tests. No command calls it yet.
   About 1,200 to 1,700 lines, most of them tests.
2. **PR 2, commands.** `fit` in `pane_geometry`; the layout commit queue
   and its `Layout` and `LayoutCommitted` worker hook stages; the in-flight
   gate; `arrange_panes` with `ArgumentKind::Layout` over the built-ins,
   including its dropdown in the Settings keybinding editor;
   next, previous, and balance with their default bindings; menus;
   notices; undo recording through #256; and the tier 1, tier 2, menu, and
   native tests. Docs: `docs/keybindings.md`, `CONTEXT.md`, and the
   guides.
3. **PR 3, user-defined layouts.** `[panes.layouts]` and `layout_cycle` in
   `huterm-config` with validation, the regenerated schema, name
   resolution and the picker over user layouts, the Settings group, and
   the config, keymap, palette, tier 2, and Settings tests. Docs:
   `docs/configuration.md` and `docs/keybindings.md`. About 1,000 to
   1,500 lines.

PR 2's undo recording uses `WorkspaceView::record_layout_change`, which
the client PR of #256 (#281, merged) added, and PR 2 opens after PR 1
merges. PR 3
waits for the shared UI component library (`huterm-ui`, approved in
`docs/plans/ui-components.md`) and builds on it instead of its own
controls: the cycle list on its `Listbox` (step 2), the name and template
fields on `Input` and `InputGroup` (step 4), and rename and the refusal
prompts on the `Field` and `InputGroup` composition and `AlertDialog` of
step 7, which also adds quake profile rename (#277). Its "Settings window"
section is revised against those components when PR 3 starts.

## Documentation

- `CONTEXT.md`: **Named layout** (a built-in or user-defined layout that
  arranges a tab's panes), **Layout template** (the text that describes a
  named layout for any pane count), and **Balance** (equal weights at every
  split of a tab, keeping its shape).
- `docs/agents/core-runtime.md` (PR 1): `arrange_panes` conserves panes,
  guards structure, and leaves the revision alone, with `reserve_ids` as
  the one allocation that does not advance it.
- `docs/agents/desktop-client.md` (PR 2): the layout commit queue, the
  fit-before-commit rule, and why arrangements gate on their own in-flight
  count rather than `busy`.
- `docs/keybindings.md` (PR 2, PR 3): the commands, the cycle and balance
  bindings, and bindings with a `layout` argument.
- `docs/configuration.md` (PR 3): `[panes.layouts]`, `layout_cycle`, and
  the template language with examples.
- This plan's `Status:` in each PR.

## Settled decisions

Jim decided these on 2026-10-27: items 1 to 8 after the first three
review rounds, and items 9 to 12 after the fifth. Each item names the
alternatives he considered.

1. **Main pane:** the first pane in traversal order, as in tmux. Not
   chosen: the focused pane.
2. **Main share:** 60 %, written into the built-in templates. Not chosen:
   a cell count like tmux's 80 columns, or a setting. User templates set
   their own shares.
3. **Tiled grid:** columns first, `ceil(sqrt(n))` columns, with the last
   row's panes sharing its width. Not chosen: tmux's rows first, or an
   aspect-ratio rule.
4. **Default bindings:** a tmux-style cycle key, `cmd-ctrl-]` and
   `cmd-ctrl-[` on macOS and `ctrl-super-alt-]` and `ctrl-super-alt-[` on
   Linux, plus balance on `cmd-ctrl-=` and `ctrl-super-=`. Not chosen:
   balance alone.
5. **Commands:** one `arrange_panes` command with a `layout` argument,
   plus next, previous, and balance, so user layouts work like built-ins.
   Not chosen: one command per built-in.
6. **Balancing in a window too small for the tab's tree:** the
   proportional fallback, which never refuses. Not chosen: refusing with a
   notice.
7. **Several `*` slots in one template:** allowed. The remaining panes are
   divided among them as evenly as possible, earlier stars taking the
   extra pane. Not chosen: at most one `*`.
8. **User-defined layouts:** in #30, as PR 3, in the config file and the
   Settings window, written in the template language. Not chosen: a
   follow-up issue, or nested TOML tables.

9. **Default cycle:** the five built-ins only; users add their own layouts
   to `layout_cycle` by name. Not chosen: appending every user layout
   automatically.
10. **A template without `*` and more panes than slots:** unavailable for
    that count, as tmux's custom layout strings apply only at their own
    pane count. Not chosen: appending the extra panes to the template's
    last split.
11. **Settings preview:** none in PR 3. Not chosen: a drawing of the
    template expanded for four panes.
12. **Renaming or removing a layout a keybinding uses:** refused in
    Settings with a message naming the binding, as the quake profile
    editor refuses to delete a profile a global shortcut uses. Not chosen:
    rewriting the bindings in the same save, or letting them show the
    unknown-layout notice.

## Open questions

None.

[issue-12]: https://github.com/jimeh/huterm/issues/12
[issue-30]: https://github.com/jimeh/huterm/issues/30
[issue-256]: https://github.com/jimeh/huterm/issues/256
[tmux-layout-set]: https://github.com/tmux/tmux/blob/523ac69601e1ea51a8ba718d32df6a00e4d0f6d4/layout-set.c
[ghostty-split-tree]: https://github.com/ghostty-org/ghostty/blob/919dca5c848d43a489a4cb2e1feade016f0fa2db/src/datastruct/split_tree.zig
[ghostty-config]: https://github.com/ghostty-org/ghostty/blob/919dca5c848d43a489a4cb2e1feade016f0fa2db/src/config/Config.zig
