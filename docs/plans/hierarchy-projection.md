# Hierarchy projection

Status: implemented for [issue #98][issue-98] in PR #199.

Core publishes an ordered stream of structural events for sessions,
workspaces, and tabs. The desktop client keeps one projection of that
hierarchy, updated from the stream on the UI thread, and renders tab labels,
the tab bar, the window model's published titles, the Quit confirmation, and
the command palette's pickers from it. The per-view `huterm_core::Tab`
snapshot goes away, and so does the palette's background hierarchy fetch.

The stream and its reducer are the client-side contract that
[#44][issue-44] serializes. Parent epic: [#9][issue-9]. Dependencies
[#20][issue-20] (session ownership) and [#97][issue-97] (client window model)
have landed.

## Current state

Observed on `main` at `5c701344`:

- Core publishes no structural events. `Mux::revision`
  (`crates/huterm-core/src/mux.rs`) advances on every structural change, but
  also on every identity allocation and on attach and detach. Its only reader
  is close-ticket staleness in `mux/lifecycle.rs`. The only event stream is
  the per-terminal, coalescing `TerminalEvent` queue in
  `crates/huterm-core/src/events.rs`.
- A successful runtime command never reaches the client. `run_on_runtime`
  (`crates/huterm-gpui/src/desktop/windows.rs`) runs `DesktopRuntime::execute`
  on a worker and handles only errors.
- `TabView` keeps the `huterm_core::Tab` returned at spawn. `TabView::label`
  reads the custom name from that record, and `refresh_tab` publishes the
  result to `WindowModel::set_tab_title`. After a core rename, the tab bar,
  the native window title, notice sources, and the Quit confirmation all show
  the old name. The tab bar calls `TabView::label` once per tab on every
  render.
- `open_palette_request` calls `load_palette_hierarchy`, which locks Mux on a
  worker through `DesktopRuntime::palette_snapshot`. Until the snapshot
  arrives, the palette holds `hierarchy: None` and defers Enter through
  `Pending::{Confirm, Commit}`; a failed fetch calls `hierarchy_failed`.
  `PaletteHierarchy::from_snapshot` takes `live_titles` to cover the stale
  records, but only for the opening window's tabs. Other windows' tabs fall
  back to the launched program name.
- Structural operations reachable from the UI: `open_tab`, including session
  and workspace creation on a window's first spawn and its rollback;
  `cleanup_spawn`; `commit_close` (detach, session, tab, tabs, application);
  drag `reorder_tab`; and the `rename_tab`, `rename_workspace`, and
  `rename_session` runtime commands. `move_tab`, `move_workspace`,
  `retarget_attachment`, and `close_workspace` have no UI caller; only tests
  use them.
- Each desktop window creates a private session. No two windows share a
  session yet, but the palette lists every session, workspace, and tab, so
  one window can rename another window's tab, workspace, or session today.

## Scope

In scope:

- Protocol types for hierarchy records, events, and a sequenced state, plus
  a reducer that applies events to that state.
- A core publisher: every committed structural mutation emits one event
  under the structural lock, and a subscription starts from an atomic
  snapshot.
- One client projection on the `Desktop` global, beside `WindowModel`, fed by
  one application-level drain, and one level-triggered reconcile function
  that brings window views in line with it.
- Runtime results carry the hierarchy sequence they committed at, and
  completions wait on that sequence instead of on lock ordering.
- Tab labels, published window-model titles, and the palette read names from
  the projection.
- The palette builds its pickers synchronously and refreshes them while open.
  `palette_snapshot`, `load_palette_hierarchy`, `Pending`, `hierarchy_failed`,
  and the `live_titles` parameter are removed.

Out of scope, with the issue that owns each:

| Deferred work | Owner |
| --- | --- |
| Attachment events in the stream | [#22][issue-22] |
| Creating a view for a tab the window did not spawn, such as a tab moved into its workspace | [#22][issue-22] |
| Closing or retargeting a window whose session or last tab was removed by another client | [#21][issue-21] |
| Wire codec, version negotiation, server incarnation, in-band lag and digest messages, and transport resync | [#44][issue-44] |
| Converging the restore-capture `HierarchySnapshot` with the protocol state | [#42][issue-42] |
| Workspace switching UI and pane split events | [#28][issue-28] and the workspace plan's follow-ups |

## Principles

Huterm avoids unnecessary CPU work, wakeups, and renders. Reliability and
efficiency both constrain this design, and these rules settle conflicts
between them:

- **Reactive, never polled.** Nothing checks on an interval whether
  something changed. Every step runs because an event reported a change:
  the drain wakes on the subscription's capacity-one channel, completions
  resume when the projection reaches their sequence, and title consumers
  refresh when a title they show changed. This plan adds no timer. #44's
  contract items below are event-driven as well; their only timer is a
  one-shot request deadline, armed only while a request waits, following
  the existing earliest-deadline pattern for genuine deadlines.
- **Idle costs nothing.** With no structural change there is no wake, task
  turn, or render.
- **Work scales with the change.** A drained batch records what it touched,
  and reconciliation visits only the windows showing touched workspaces or
  tabs. Every client write compares before writing, and a view is notified
  only when something it renders changed. A no-op batch causes no render.
- **Hot-path reads are constant time.** The tab bar resolves every tab's
  label on every render. The projection indexes tabs by ID, and label
  resolution borrows names from the projection without allocating more than
  it does today.
- **Correctness never depends on observing a particular event.** Events
  record what changed and which windows to visit; the reconcile function
  derives views from projection state. A missed or coalesced event cannot
  leave a window wrong once its state is current.
- **Loss is detected, never silent.** Sequence numbers expose gaps, and a
  detected gap leads to snapshot recovery. Expensive verification, such as
  state digests, stays out of steady state: tests and debug assertions use it
  now, and #44's checkpoints use it occasionally.
- **The core publisher stays cheap under the Mux lock.** With no subscriber,
  emission builds nothing. With subscribers, it clones one small event per
  subscriber into a bounded queue and signals a capacity-one channel; it
  never blocks or waits on a subscriber.

## Decisions

### The reducer lives in `huterm-protocol`

`huterm-protocol` already owns dependency-neutral IDs, lifecycle events, and
immutable snapshots, and `mise run architecture` keeps its dependency set
empty. The hierarchy events, records, and reducer fit there. #44 serializes
these types, a future TUI client reuses the reducer, and core can test its
own state against the reducer's output.

The types are plain data that a codec can represent: scoped IDs, strings,
`u32` indexes, and a `u64` sequence. Use `u32`, not `usize`, for positions so
the contract does not change width on `wasm32-unknown-unknown`. No `serde`
derive yet: #44 chooses the codec and updates the dependency policy
deliberately.

`HierarchyState` keeps sessions in order, workspaces and tabs in hash maps
keyed by ID, and per-parent order vectors. Tab lookup by ID, the label hot
path, is constant time. Reordering within one parent costs that parent's
length, which is small.

### A contiguous sequence, separate from `Mux::revision`

Each event carries a sequence number that advances by exactly one per emitted
event. `Mux::revision` cannot serve, because it skips values on allocation and
on attach and detach. With contiguous numbers the reducer distinguishes three
cases:

- `seq <= applied`: a stale or duplicate event. Ignore it and report
  `Stale`.
- `seq == applied + 1`: apply it.
- `seq > applied + 1`: a gap. Change nothing and report `ResyncRequired`.

The sequence belongs to the Mux, not to a subscription, and advances even
with no subscriber, so a later snapshot is always comparable. A
resubscription snapshot carries the sequence it reflects, so events queued
before it are rejected as stale and events after it apply in order.
`Mux::hierarchy_seq()` reports the current value.

Every event envelope carries a `StreamId` alongside the sequence, and the
state records the stream it describes. Today `StreamId` wraps the `RuntimeId`;
[#44][issue-44] extends it with server incarnation without changing any
event. The reducer validates the envelope's stream for every variant,
including `Reset`, which has no IDs of its own, and rejects a mismatch like a
gap.

Core keeps `Mux::revision` for close-ticket validation unchanged.

### Small, state-like events with absolute positions

```text
SessionCreated   { session: SessionInfo, index }
SessionRenamed   { session, custom_name }
SessionClosed    { session }                 cascades to workspaces and tabs
WorkspaceCreated { workspace: WorkspaceInfo, index }
WorkspaceRenamed { workspace, custom_name }
WorkspaceMoved   { workspace, session, index }
WorkspaceClosed  { workspace }               cascades to tabs
TabOpened        { workspace, tab: TabInfo, index }
TabRenamed       { tab, custom_name }
TabMoved         { tab, workspace, index }   covers same-workspace reorder
TabClosed        { tab }
Reset                                        application shutdown
```

`SessionInfo`, `WorkspaceInfo`, and `TabInfo` carry identity, custom name,
and the core-owned automatic or fallback name. `TabInfo` also carries the
pane and terminal IDs. A moved or reordered item's `index` is its final
position, so applying an event never depends on an anchor that a client
might resolve differently. Closing a parent emits one event; the reducer
removes its children.

Core emits when structure commits, and emits nothing for an operation
rejected before mutation or for a no-op. A committed removal emits even when
the terminal cleanup that follows it fails: `close_tab`, `close_workspace`,
`close_session`, and `shutdown` all remove structure before their fallible
cleanup and then return the cleanup error. Emission must precede that
cleanup, or the projection keeps an object core no longer has. Rollback paths
need no special case: a failed first spawn emits `SessionCreated`,
`WorkspaceCreated`, then `SessionClosed`.

Tab name resolution (custom name, then a non-blank terminal title, then the
fallback name) moves to one protocol function that both `huterm_core::Tab`
and `TabInfo` use, so the client cannot drift from core's rule.

### Bounded subscription with resync

`Mux::subscribe_hierarchy()` returns a `HierarchyState` snapshot and a
subscription atomically under `&mut Mux`. No event falls between the
snapshot and the stream. The subscription has a bounded queue and a
coalescing wake, matching the shape of `EventPublisher`:

- `try_recv` returns the next event, `Empty`, or `Lagged`.
- When the queue would overflow, the publisher discards the queue and marks
  the subscription lagged. The subscriber resubscribes from a worker, which
  replaces its state and subscription together.
- Dropping the subscription unregisters it. A publisher never blocks on a
  subscriber.

Installing a resync snapshot has three steps:

1. Replace the state and the subscription together.
2. Drain the new subscription. A spawn that committed after the worker took
   the snapshot may already have installed its view; its `TabOpened` waits
   in this queue, so draining first keeps that tab from looking removed.
3. Reconcile every window, then release sequence waiters (see "Results carry
   their committed sequence").

If the new subscription lags again before step 3, skip the reconcile and
resync again; never reconcile membership against a state with lost events.

Overflow is not expected in process, but #44 needs this exact path for slow
consumers, and building it now makes it testable before a transport exists.
The queue bound is a named constant sized well above one burst of UI
operations.

### Structural and lifecycle streams are ordered independently

The hierarchy stream has a total order. Each terminal's `TerminalEvent`
stream keeps its own order. There is no order across them. A total order
would need one unified bus, and that conflicts with per-terminal coalescing.
The client tolerates any interleaving:

- Labels read the projection when they are resolved, so a rename and a title
  change can arrive in either order.
- `TabClosed` retires a tab's view whatever terminal events remain queued
  for it.
- A terminal event for a tab the projection no longer holds is ignored.

This is the reading of the issue's "ordered with the existing lifecycle
events" that this plan adopts. #44 can carry both streams on one connection
without inventing a cross-stream order.

### One projection and one drain on the client

`Desktop` gains a `hierarchy` field holding the projection's state, its
subscription, and the pending sequence waiters, beside `windows:
WindowModel`. Core structure and per-window facts stay separate: the
projection never stores window navigation, and the window model never stores
names.

`DesktopRuntime` subscribes when the application starts, before any window or
worker exists, so the UI thread takes the Mux lock only while it is
uncontended. After that the UI thread never locks Mux for the projection.

One application-level task waits on the subscription's wake. When woken, it
drains every queued event in one batch on the UI thread, then reconciles.
Per-window drains would apply each event once per window. The drain is also
callable directly from application context, which completions use (see
below). It must release its `Desktop` borrow before updating any view.

### Level-triggered reconciliation

Applying a batch returns a `Touched` summary: workspaces whose tab
membership or order changed, tabs whose names changed, and whether any
session or workspace name changed. Recovery uses a summary that touches
everything. The reducer builds the summary while it applies events, so
finding the affected windows costs nothing extra.

`reconcile(touched)` visits only windows whose workspace or shown tabs
appear in the summary. For each one it derives the desired state from the
projection and converges the window's views and `WindowModel` record:

1. **Membership.** Drop installed views whose tabs the projection no longer
   holds, through `drop_tab_views`. A view records the sequence its tab
   committed at; a tab is removed only when the projection has applied that
   sequence and no longer holds it. A view the projection has not caught up
   with is never dropped. While the window is busy, removal waits: the
   window's own structural completion owns that aftermath, and dropping a
   focused view on a separate drain turn would lose keystrokes until the
   completion refocuses. Each completion reconciles its own window after
   clearing `busy`, so a deferred removal still applies.

   A projected tab with no installed view is normal while the window's own
   spawn is in flight: core emits `TabOpened` before the worker returns, and
   both the wake-driven drain and the completion's own drain reconcile
   before `push_tab_view` runs. The window's existing `busy` flag is set for
   the whole spawn and cleared only inside the completion's view update, so
   reconcile leaves such tabs alone while the window is busy. A projected
   tab with no view in a window that is not busy is the unsupported
   external-open flow: leave it and fire a `debug_assert`. Reconcile never
   installs views.
2. **Order.** Compute the installed order (see "Tab order follows the
   projection") and apply it only when it differs from the current order.
3. **Titles.** Recompute labels for touched tabs and publish them through
   `set_tab_title`, which reports whether anything changed.
4. **Notify.** Notify the window only if a step changed something it
   renders.

After visiting windows, reconcile refreshes title consumers if their inputs
changed (see below).

Because reconcile derives its writes from state, it is idempotent: running
it twice, or after a window's own completion already applied the same
change, writes nothing and notifies nothing. Events only narrow which
windows to examine; a missed event cannot leave a reconciled window wrong.
This one function serves normal batches, recovery, and every completion, and
replaces the per-event effects table of earlier drafts.

After application teardown commits, which the runtime's existing
`terminating` gate records, reconcile skips membership changes: Quit owns
teardown. `Reset` arrives only then. Before that point, including while Quit
assesses, confirms, or is cancelled, reconcile runs normally:
`Desktop.quitting` is set as soon as Quit is requested and does not mean
teardown has begun.

### Title consumers

Title consumers are the views that display names they do not own: open
palettes, and windows showing a close confirmation, whose dialog reads the
model's titles only when it renders. Reconcile refreshes them only when
their inputs changed. A palette's rows copy names, membership, per-workspace
positions, and parent details, so any non-empty `Touched` summary refreshes
an open palette; structural batches are rare, and the rebuild compares rows
before notifying. A confirmation host refreshes when a title within its
scope changed. Refreshing means rebuilding the open palette's hierarchy
and notifying the confirmation host. It never changes a confirmation's
assessment, generation, focus, or busy state. A Quit dialog in window 0
therefore follows a rename of window 1's tab made after the dialog opened.

Terminal-driven titles reach consumers through the same path without a
structural event. `refresh_tab` runs inside a `WorkspaceView` update, so it
cannot refresh consumers directly: `views::broadcast` would re-lease that
view. When `set_tab_title` reports a change and a title consumer exists,
`refresh_tab` sets a dirty flag on `Desktop` and, if the flag was clear,
defers one application-level refresh with `cx.defer`. With no open palette
or confirmation, it does nothing extra. A title flood across many tabs then
costs at most one consumer refresh per deferred turn, on top of the activity
drain's own per-terminal coalescing.

### Results carry their committed sequence

Completions must not assume the projection is current because core emitted
under the same lock. Over IPC, a command result and its events can arrive in
either order. Every structural `DesktopRuntime` operation (spawn, close
commit, reorder, runtime commands, and spawn cleanup) returns
`Mux::hierarchy_seq()` read under the lock before releasing it, whether the
operation succeeded or failed after committing. A failed close cleanup still
committed its removal, so its error carries the sequence too.

A completion first ensures the projection has applied its committed
sequence, then acts:

- In application context, before entering `view.update_in`, it drains. In
  process this always satisfies the sequence, so the completion continues on
  the same turn with no added wait or task.
- If the projection is still behind, the completion registers a one-shot
  waiter keyed by its sequence. In process this happens only while a resync
  is pending; over IPC it also happens whenever a result arrives before its
  event. No completion polls.

Every point where the projection advances releases the waiters it
satisfied: the end of each drain and reconcile, and the end of resync
installation. Waiters are ordered by sequence, so release pops from the
front and costs nothing when none are pending.

A waiter is a one-shot channel. Release sends on it, which only wakes the
waiting completion task; the task resumes on a later executor turn, after
the releasing code has returned its borrows. Release never runs completion
code inline.

Each waiter resolves exactly once, with `Ready` or `Cancelled`, and is never
dropped unresolved. Resolution removes the waiter before sending, so a
competing release and cancellation cannot both resolve it.

- Teardown commit (`terminating`) cancels all waiters. A stream change,
  which only #44 can produce, cancels them too.
- In process, resubscription cannot fail: the worker takes the Mux lock,
  which recovers from poisoning, and subscribes. Over IPC, a failed resync is
  a disconnect, which cancels waiters until #44's reconnect resumes.
- The application-scope close completion and `terminate` do not wait. Their
  commit is teardown itself, which cancels every waiter; they keep today's
  flow, including `approved_quit`, unchanged.

After resolution, a completion takes exactly one of three paths, and each
path owns its cleanup once:

- **Ready, view alive:** publish the result as today.
- **View gone, either resolution:** the completion task is detached from the
  window, so it resumes and runs today's missing-view path, such as
  `cleanup_spawn` for a successful spawn.
- **Cancelled, view alive:** run the operation's existing failure aftermath
  without publishing the result: clear `busy`, resume queued closes, and
  keep generation checks. A spawn also runs `cleanup_spawn` unless teardown
  has committed, because teardown already owns every terminal. A failure
  notice appears only when the application is not terminating.

`pending_spawns` keeps today's meaning: a spawn is pending until its result
is published or its failure path finishes. The decrement moves to the end of
the completion, after the wait and after publication or cleanup, and runs
exactly once on every path. Quit therefore still waits for spawns that are
waiting on the projection, and `quit_pending` resumes Quit only after the
last spawn settles.

Waiting is bounded by the resync worker's Mux acquisition, the same bound
every structural operation already has. In exchange, a completion never acts
against a stale projection, and the earlier plan's "reconcile corrects what
completions published" case disappears. Over IPC, the same rule gives each
client read-your-writes without other changes.

### Membership: reconcile removes, completions own aftermath

`push_tab_view` stays on the spawn completion, because creating a view needs
the `RuntimeClient` that only the spawn result carries. `TabOpened` updates
only the projection. The committed-sequence wait guarantees the projection
holds the tab before the view is pushed, and the view records that sequence
for the membership rule above.

For order, reconcile is authoritative; for removal, it is authoritative
except in a busy window, whose completion removes. A window's own close
completion keeps the aftermath it already owns in `remove_closed_tabs`:
dropping the closed views, then selecting and focusing the next tab in the
same update, reconciling the rest of the window, resuming queued closes,
and closing an emptied window.

Drag reorder no longer needs the worker to return an order.
`DesktopRuntime::reorder_tab` returns the committed sequence, and the
completion relies on reconcile. The "Tab order changed before reorder
completed" warning goes away with the returned order.

Today, only a window's own close flow removes its tabs. A removal with no
local completion, or a `TabOpened` in a shown workspace without a local
spawn, cannot happen yet. Reconcile still removes the view in the first case,
because removal is derived from state; the window's aftermath for an
externally emptied window belongs to [#21][issue-21]. The second case leaves
the projection correct, installs nothing, and fires a `debug_assert` until
[#22][issue-22] defines it.

The palette smoke's `delete-target` command
(`crates/huterm-gpui/src/desktop/palette_smoke.rs`) closes a displayed tab
directly through Mux, without a local completion, to prove stale-target
rejection in `check-palette.ts`. That is the external-removal case above, and
it also deletes while the identity picker is active, which live refresh now
changes (see the palette section). Rework the fixture and its smoke step:

1. The fixture creates a session and workspace through Mux that no window
   displays.
2. The smoke runs `rename_workspace`, selects the fixture workspace in the
   workspace slot, commits it with Tab, and then runs
   `palette_previous_slot` to return to the name slot. Navigating back does
   not commit, so the Tab must come first. The identity is now a committed,
   inactive value.
3. The fixture closes the workspace. The active name slot has no picker to
   watch, so smoke state publishes the palette's applied hierarchy sequence
   and whether its workspace domain still lists the fixture workspace. The
   smoke waits until the domain omits it while the slot keeps its value.
4. The smoke submits and asserts `StaleTarget` and that every surviving
   name is unchanged.

### Tab order follows the projection

A window's installed views can lag the projection while a spawn's view is
still being created. `apply_tab_order` accepts only an exact permutation, so
applying the workspace's full projection order `[B, A, C]` to installed
views `[A, B]` is rejected, and a later append of C would leave `[A, B, C]`
for good.

The window's installed order is the projection's tab order for its
workspace, filtered to the tabs the window has installed, followed by any
installed tab the projection does not hold, in their current order. It is
always a permutation of the installed views, so `apply_tab_view_order`
accepts it. A tab can be missing from the projection only briefly, while a
resync is pending, or through an unsupported flow.

Every membership change re-applies the installed order:

- `push_tab_view` appends the view and its `WindowModel` entry, as it does
  today, then applies the installed order to both, so the alignment
  assertion holds before and after the reorder.
- `drop_tab_views` applies the installed order after removing, so a close
  completion that lands after a missed reorder still converges.
- Reconcile applies it for touched workspaces.

Each application compares first and writes only when the order differs.

### Labels and the window model read the projection

`TabView.record` becomes `terminal: TerminalId` plus the committed sequence,
the spawn parameters the view needs. `TabView::label` looks the tab up in
the projection by ID in constant time and borrows its custom and fallback
names. A tab missing from the projection resolves to the live terminal title
alone. Label resolution allocates no more than it does today.

Titles still reach `WindowModel` through `set_tab_title`. `refresh_tab`
publishes terminal-driven changes, and reconcile publishes rename-driven
ones. The Quit confirmation and Quit capture keep reading the window model.
Capture needs no change; an open confirmation dialog redraws through the
title-consumer refresh.

### The palette reads the projection synchronously

`open_palette_request` builds `PaletteHierarchy` from `Desktop.hierarchy`
and resolves the target's session from it. Tab rows take their label from
the projection and the open windows' published titles, so every window's tabs
show live titles, not only the opening window's. The `Pending` state, the
loading branch in `DomainView::rows`, `hierarchy_failed`, `palette_snapshot`,
`load_palette_hierarchy`, and `live_titles` are removed.

While the palette is open, the title-consumer refresh rebuilds its
hierarchy, so picker labels follow terminal titles as well as renames in
either order. Rebuilding replaces `refresh_palette`'s availability-only
update for this purpose. Pickers keep the selected value when it still
exists. A committed slot whose target disappears keeps its value, so Enter
still reaches the executor and fails with `StaleTarget`.

A highlighted row whose value disappears must not hand Enter to another
target. Today `picker_index` falls back to row 0 when the highlighted value
is missing, which is harmless while rows never change under the user. With
live rebuilds it would silently retarget the pending command. After a
rebuild removes the highlighted value, the picker shows no highlight, and
Enter does nothing until the user moves the highlight or types a filter.
The row-0 fallback stays for the picker's initial selection and after the
user changes the filter. `move_selection` currently needs an existing
index; with no highlight, Down selects the first row and Up the last.

The existing prefill rule cannot run on every rebuild. `prefill_name` treats
empty text as untouched, so a rebuild after the user deliberately blanked the
name would restore the old override, and Enter would no longer clear it. The
name slot tracks whether its text is automatic or user-edited. A rebuild
refreshes automatic text, including a nonempty prefill that another window's
rename made stale, and never touches user-edited text, including an edited
blank.

`DesktopRuntime::execute` keeps resolving `rename_session`'s session under
the Mux lock. That lookup is canonical runtime state, and the projection
could be one event behind.

### Contract items for #44

The in-process client needs none of these, but the envelope and state must
leave room for them so #44 adds messages instead of changing events:

- **Loss is an event, so no heartbeat.** A gap is visible only when a later
  event arrives, so a design that could drop the last event silently would
  need a periodic head-sequence heartbeat. This one cannot drop it silently.
  The local transport is a reliable, ordered byte stream, so within one
  connection events are never lost or reordered. Every way to lose events
  surfaces as an event the client already handles: queue overflow sends an
  in-band `Lagged` message in place of the discarded events, and peer exit
  or a broken connection reports end-of-stream or an error. On either, the
  client reconnects or resubscribes from its last applied sequence and
  recovers by snapshot. The in-process client has the same properties
  without a transport.
- **Digest checks ride on existing messages.** A complete stream can still
  diverge through a bug in emission or the reducer. The server attaches
  `{seq, digest}` to messages it already sends at natural boundaries: every
  subscription snapshot, every command result, and every Nth event, with N
  a named constant. Counting events is event-driven; no timer decides when
  to check. A client whose projection digest differs at that sequence
  resyncs. The digest is a specified, stable hash, such as 64-bit FNV-1a over
  a canonical encoding of the state; std's `DefaultHasher` does not promise
  stable output. Computing it costs time proportional to the hierarchy, so it
  never runs per event.
- **Server incarnation in `StreamId`.** `RuntimeId` is unique only within
  one process, so a restarted server could reuse it with a reset sequence.
- **Request deadlines, not liveness probes.** A hung server can keep its
  socket open, and the socket reports hangup only when the peer closes, so
  no event ever reports the hang. Bounded progress therefore needs a
  deadline, but only while something waits: a client arms a one-shot
  deadline when it sends a request or registers a sequence waiter, and
  disarms it on the result. Idle connections arm nothing. This is the
  existing earliest-deadline pattern for genuine deadlines, not a poll.

This plan specifies `HierarchyState::digest()` and uses it in tests and debug
assertions, but attaches no digest to any message.

## Implementation steps

One PR with two implementation commits, each independently reviewable.

1. Protocol and core.
   - Add `StreamId`, `HierarchyState` with its ID indexes, the three info
     records, `HierarchyEvent`, the sequenced envelope, the apply outcome
     with its `Touched` summary, the reducer, `digest()`, and the shared
     tab-name function to `huterm-protocol`.
   - Add the publisher and `hierarchy_seq()` to `Mux`, and emit from
     `create_session`, `create_workspace`, the three renames, `open_tab`,
     `move_tab` (and so `reorder_tab`), `move_workspace`, `close_tab`,
     `close_workspace`, `close_session`, and `shutdown`.
   - Add `subscribe_hierarchy`.
   - No client change.
2. Client.
   - Add `Desktop.hierarchy`, the startup subscription, the drain task,
     sequence waiters, and the resubscribe worker.
   - Add `reconcile` and route spawn, close, reorder, and runtime-command
     completions through the committed-sequence wait.
   - Replace `TabView.record`; resolve labels from the projection.
   - Build the palette synchronously; refresh title consumers; remove the
     pending and fetch machinery.
   - Update documentation and smokes.

## Verification

Protocol reducer unit tests:

- Creation, rename, move, and close for each level, including cascading
  closes and `Reset`, each with the expected `Touched` summary.
- A stale or duplicate sequence changes nothing and reports `Stale`. This is
  the issue's out-of-order criterion.
- A gap or a foreign-stream envelope changes nothing and reports
  `ResyncRequired`. Cover a foreign `Reset` carrying the next expected
  sequence, which only the envelope check can reject.
- Two projections seeded from the same snapshot and fed the same stream end
  equal, with equal digests. This covers the criterion that two windows on
  one session observe the same changes in the same order; shared attachments
  arrive with #22.
- The digest is stable for equal states and changes for each kind of
  difference: names, order, membership, and parentage.

Core tests:

- A differential test drives every emitting operation, including the moves,
  `close_workspace`, rollback, and `shutdown`, and after each step asserts
  that the reducer's state equals a fresh `subscribe_hierarchy` snapshot.
  Perturb one emitter once, such as dropping `index` from `TabMoved`, and
  confirm the test fails at its equality assertion.
- Rejected and no-op operations emit nothing and leave the sequence
  unchanged.
- A committed removal whose terminal cleanup fails still emits its event, if
  an existing fixture can force that cleanup failure. Otherwise, keep
  emission before the fallible cleanup in code and say so in review.
- Overflow reports `Lagged`. Resubscription converges: events queued before
  the new snapshot are rejected as stale, and later events apply.
- The equality check alone cannot prove delivery, because a test can apply
  events it collected itself. Read events through the real subscription,
  including its wake signal.
- With no subscriber, operations allocate no events.

Client checks:

- Reconcile unit tests, asserting views, `WindowModel` entries, and
  notification counts:
  - An empty `Touched` summary visits no window and notifies nothing.
  - Reconciling twice, or after a completion already applied the change,
    writes and notifies nothing the second time.
  - A batch of several renames in one window notifies that window once;
    windows outside the summary are not visited.
  - A view whose committed sequence the projection has not applied is never
    dropped.
  - The real spawn path, draining before `push_tab_view`, reconciles a
    projected tab with no view in a busy window without asserting; the same
    state in a window that is not busy asserts in debug builds.
- Installed-order unit tests:
  - A projection order that already contains a tab whose view is not pushed
    yet, followed by the spawn completion, converges on the projection's
    order. The reverse arrival order converges too, including a non-tail
    insertion.
  - A close completion that lands after a missed reorder converges.
- Committed-sequence tests:
  - A completion whose sequence is already applied proceeds on the same
    turn.
  - A result that arrives before its event, with no gap or resync, waits
    and is released by the next ordinary drain.
  - One behind a pending resync waits and is released by installation.
  - Teardown commit cancels pending waiters. A cancelled spawn whose view
    survives clears `busy`, resumes queued closes, and runs no
    `cleanup_spawn` once teardown has committed; before teardown, a
    cancellation runs `cleanup_spawn` exactly once.
  - A window that closes while its completion waits still gets its
    missing-view path, with cleanup counted exactly once.
  - A release and a cancellation racing for one waiter resolve it once, and
    the completion's cleanup runs once.
  - A spawn waiting on the projection keeps `pending_spawns` nonzero, so
    Quit requested from a sibling window sets `quit_pending` and proceeds
    only after the spawn publishes or fails.
  - The application-scope close completion does not wait, and
    `approved_quit` still runs after committed teardown.
- Resync installation tests:
  - A snapshot taken before a spawn commits, installed after that spawn's
    view: draining the new subscription adds the tab, so reconcile removes
    nothing and fires no assertion.
  - A new subscription that lags again before reconciling triggers another
    resync instead of a membership pass.
  - The membership skip applies only once `terminating` is set. Cover
    `quit_pending`, Quit assessing, Quit confirming, cancelled Quit, and
    committed teardown separately; only the last skips membership.
- Title-consumer tests: a reset event before the terminal title change, and
  a title change before the reset, each leave the published title and an
  open palette's row on the final label. With no consumer open, a title
  change schedules no deferred refresh. A reorder or move with no rename
  updates an open picker's order, positions, and parent details, keeping the
  highlighted identity.
- Palette unit tests for the name slot: a rebuild refreshes an automatic
  prefill after an external rename and preserves a user-edited blank.
- Palette unit tests for live picker rebuilds: removing the highlighted
  value leaves no highlight and Enter commits nothing; Down and Up then
  select the first and last rows; changing the filter restores the initial
  selection; removing a committed slot's value keeps it for the executor to
  reject.
- Synchronous palette loading is enforced by type: the palette's hierarchy is
  a required constructor argument, not an `Option`, so no code path can
  show an unloaded picker.
- `check-palette.ts`: model titles alone can pass while the tab bar stays
  stale, so assert render-bound evidence too. `w{n}.window_title` comes from
  `sync_window_title` during render and covers the active tab. Add the tab
  bar's rendered labels to smoke state if that proves insufficient.
  - Rename the active tab and assert the model title and `w0.window_title`.
  - Rename window 1's active tab from window 0's palette and assert
    `w1.window_title` without sending window 1 any input.
  - Reset the name and assert the terminal title returns in both.
  - With the palette open on the tab picker, rename the tab from another
    window and assert the picker row's label changes without reopening.
  - Keep stale-target rejection through the reworked `delete-target`
    fixture and its committed-slot sequence.
- `check-overlays.ts`, which runs inside the palette smoke: open the Quit
  dialog in window 0, then rename the busy sibling tab in window 1, and
  assert the dialog's group heading changes. `dialog_smoke_fields`
  recomputes the dialog from the model rather than reading what rendered, so
  record the groups the last render drew and assert those instead.

Efficiency gates:

- `mise run ci:benchmarks`: output latency, keystroke echo, scroll, and link
  budgets unchanged. The keystroke mode exercises `refresh_tab`, which gains
  only a branch when no title consumer is open.
- `mise run bench:idle` on macOS, before and after: idle wakeups and CPU
  unchanged. It needs an unlocked, unoccluded screen.
- Tab bar label resolution. No existing benchmark reaches it:
  `bench:renderer-scenarios` measures terminal `prepare` and `paint` against
  synthetic snapshots only. Measure `TabView::label`'s resolution directly
  in release mode, before and after, at 1, 50, and 500 tabs, and report
  elapsed time and allocations per call in the PR. Promote it to a Mise task
  only if the numbers show a regression worth guarding.

Commands: `mise run check`, `mise run test`, `mise run smoke:macos-palette`
locally, and `mise run linux:smoke` for the X11 palette smoke. Run `mise run
verify` before handoff.

## Documentation

- `AGENTS.md`: replace "Desktop tab records are initial snapshots;
  propagating later core renames to views belongs with the deferred rename
  UI" with the projection rules: names come from `Desktop.hierarchy`; the UI
  thread never locks Mux for structure; completions wait on their committed
  sequence; client effects derive from projection state through reconcile.
- `docs/plans/workspaces-windows-tabs.md`: update the sentence about initial
  tab-record snapshots to point here.
- `crates/huterm-gpui/src/desktop/windows/model.rs`: update the module
  comment that says core renames "reach the model with #98".
- `CONTEXT.md`: add **Hierarchy projection** if review agrees it is a domain
  term rather than an implementation name.
- Issue #44: once this work merges, reference the protocol event types and the
  contract items above as the client-side contract. Confirm with the user
  before editing the issue.

## Risks

- Rebuilding palette rows while the user navigates can move the highlighted
  row. Keep the selection by value, not by index, and never fall back to
  another row after a removal.
- Rebuilding an open palette costs work proportional to the hierarchy. It
  happens only while a palette is open and at most once per deferred turn.
  Check that a title flood with the palette open stays responsive in the
  sibling window during the smoke, and narrow rebuilds to changed rows only
  with evidence.
- Reconcile fans out into window views from application context. It must
  release its `Desktop` borrow before updating any view and must never read
  a window that is already on GPUI's update stack. Completions drain and
  reconcile in their async task before `view.update_in`, so the fan-out never
  re-leases the completing view.
- Emitting under the Mux lock adds a short critical section to every
  structural operation. The publisher only clones a small event into bounded
  queues and signals a capacity-one channel, so it adds no blocking work.
- A completion waiting on a pending resync delays its aftermath until the
  resync worker acquires Mux. In process this needs a queue overflow first,
  which the bound makes unrealistic.

## Unresolved questions

- None blocking. The deferrals table records the work handed to later
  issues.

[issue-9]: https://github.com/jimeh/huterm/issues/9
[issue-20]: https://github.com/jimeh/huterm/issues/20
[issue-21]: https://github.com/jimeh/huterm/issues/21
[issue-22]: https://github.com/jimeh/huterm/issues/22
[issue-28]: https://github.com/jimeh/huterm/issues/28
[issue-42]: https://github.com/jimeh/huterm/issues/42
[issue-44]: https://github.com/jimeh/huterm/issues/44
[issue-97]: https://github.com/jimeh/huterm/issues/97
[issue-98]: https://github.com/jimeh/huterm/issues/98
