# Terminal viewers

Status: in progress. Approved for [issue #22][issue-22] after three
adversarial review rounds. PR 1 (#208) delivered [#205][issue-205].
[#206][issue-206] ships as two PRs, A and B (see Delivery); PR A (#225) is
in review and PR B is in progress.

A terminal can be shown by more than one view at once: two desktop windows
on one session today through a smoke fixture, and later panes, browser pages,
and TUI clients. This plan gives each view its own runtime registration, a
**viewer**, so views stop sharing one destructive event queue and one wake.
The runtime keeps one canonical grid size, one shared viewport, and one
snapshot cache per terminal, and it arbitrates size, presentation, focus
reports, and mouse gestures between viewers on its owner thread.

Parent epic: [#9][issue-9]. Dependencies [#20][issue-20], [#97][issue-97],
and [#98][issue-98] have landed. [#26][issue-26], [#28][issue-28],
[#44][issue-44], and [#134][issue-134] build on this work.

## Settled decisions

Agreed with the maintainer before drafting:

- Deliver core viewers first and desktop multi-viewer second, and move the
  smallest, largest, and manual sizing policies to a separate issue. The
  desktop half was planned as one PR and now ships as two (see below).
- Terminal status is revisioned state, not a queue of events.
- The `latest` sizing policy follows the viewer that most recently gained
  focus or sent input.
- Presentation authority follows the same controlling viewer as size.
- Mouse arbitration starts with a reasonable default behind one runtime
  policy that can change without a protocol change.
- `close_on_exit` as a runtime-owned policy belongs with [#45][issue-45] or
  the server half of [#21][issue-21]. The desktop picks one closing window
  for now.
- Attachment events in the hierarchy stream move to [#26][issue-26] and
  [#21][issue-21].
- The second desktop viewer exists only as a smoke fixture. A user-facing way
  to show one session in two windows waits for a workspace sidebar with a
  session switcher.

Agreed on 2026-10-03 for the desktop half ([#206][issue-206]), after PR 1
landed and the code on `main` at `8dfb406c` was read again:

- It ships as four GitHub stacked PRs, A to D. Changed on 2026-10-15: two
  PRs, described under Delivery. PR A is as planned and bases on `main`.
  PR B combines the former B, C, and D. They are one feature, and merging
  them separately would leave two gaps on `main` between merges: two
  windows both closing an exited tab, and a revoked view left without a
  viewer. PR A stays separate because a single viewer can use everything it
  changes, and because it carries the harness spike that PR B's tests
  depend on.
- Desktop tests use GPUI's own test helpers where they fit. Dependencies
  that only tests use are accepted.
- Smoke state keeps `grid=` as the grid a view requests and adds a separate
  field for the grid it displays.
- Cancelling an exit-close confirmation clears the owed close for every
  window. A window closed while its confirmation shows returns the close to
  owed. Changed before PR B: an exit-close never prompts, so these rules
  apply only if that changes (see the exit-close decision).
- Decided on 2026-10-21 in PR B part 2: `Desktop` does not own an
  exit-close outright. It owns the owed exit-closes, and the window close
  pipeline that takes one assesses and commits it (see the exit-close
  decision).
- In-place re-subscription (PR B) is covered through the smoke fixture only.
  The desktop never calls `retarget_attachment`, `move_tab`, or
  `move_workspace`, so a revoked viewer whose tab stays in the window's
  workspace has no production trigger until [#21][issue-21] and
  [#26][issue-26].

## Current state

Observed on `main` at `e87c6084`:

- `RuntimeClient` (`crates/huterm-core/src/terminal.rs`) is one cloneable,
  unrestricted handle. Its clones share `events: Arc<Mutex<EventReceiver>>`
  and one capacity-one `activity` channel; the `wait_for_activity` doc says
  to give it a single waiter. `Mux::attach` documents that simultaneous
  clients need broadcast. `Mux::runtime_clients` also returns clients and
  has no caller.
- `EventPublisher` (`events.rs`) coalesces titles, metadata, invalidations,
  and bells to their latest pending value and keeps `Ready`, `Exited`, and
  the first `Failed` once. A view created after an event was consumed never
  learns it.
- Invalidation is one `invalidation_pending: AtomicBool`, cleared on the
  owner thread when a snapshot is built, and one `invalidated_at` instant
  taken by that snapshot. A second viewer's snapshot would clear the first
  viewer's pending wake.
- Engine damage is already sampled once. `TerminalEngine::snapshot`
  (`engine/ghostty.rs`) calls `render.clean()` only after the retained
  `Arc<TerminalRow>` rows are coherent, and every snapshot is complete. A
  second snapshot of unchanged state reuses every row but still walks the
  render state.
- `generation` advances on output, resize, and buffer edits, not on
  scrolling or presentation changes. Selection extraction checks it.
- Scrolling happens inside the snapshot control, on the control channel
  that the owner thread drains before client messages. Nothing tells other
  views that the viewport moved. `complete_snapshot_request` treats every
  snapshot error as fatal and sets the closing gate.
- Any holder of `RuntimeClient` can call `resize`. `TerminalView`'s
  `resize_to_layout` (`crates/huterm-gpui/src/desktop.rs`) resizes on layout
  changes, skips the call when the grid and cell size are unchanged, and
  retains a refused resize in a latest-wins `pending_resize`, outside the
  `InputQueue`. `WorkspaceView::sync_tab_layout` runs it for hidden tabs
  too, so config, scale, and bounds changes reach background PTYs, as
  the desktop client guide requires. After root exit the runtime still resizes the
  emulator and skips only the PTY ioctl. Two views of one terminal would
  alternate the PTY size.
- Input, resize, presentation, and edit messages share one bounded
  `messages` queue, ordered on the owner thread. Input also shares one
  1 MiB byte budget, and the runtime discards input after root exit. Only
  `PresentationUpdate` identifies the registration that sent it.
- `TerminalView` enqueues `Focus(true)` on focus and `Focus(false)` on blur
  through its `InputQueue`, after any mouse releases that blur cancels, and
  retries them on `Busy`. The `InputQueue` rejects new entries past its
  event and byte limits, except owned releases, and closes after exit.
- GPUI focus events already follow window activation. A frame records
  whether its window is active, the focus path counts as empty while it is
  not, and focus listeners fire when either changes
  (`third-party/vendor/gpui-pre-0.3.6/src/window.rs`). Switching
  applications or windows therefore already blurs the terminal view and
  writes `ESC [ O` when focus reporting is on. Like all GPUI focus events,
  these fire on a drawn frame.
- `ScrollController::bottom`, called when the user types, replaces any
  pending scroll intent with `Live` on the next snapshot request.
- An owned mouse release is sent even when the pointer has left the grid,
  with clamped coordinates, so accepted gestures always end.
- `PresentationAuthority::authorize` (`presentation.rs`) revokes the
  previous controller. A second view would make the first view's theme
  updates fail with `Stopped`, which `publish_presentation` reports as an
  error notice.
- `HostEffectSink::select_recipient` (`host_effects.rs`) already picks one
  eligible recipient by focus ordinal, checks generations, never redirects,
  and treats `LocalText` and `Remote` origins as ineligible. Admission wakes
  the terminal's single `activity` channel, not the recipient's view.
- Each window's `ExitQueue` (`desktop/windows.rs`) records an exit-close
  only when its view observes the exit transition while `close_on_exit` is
  set, so a reload affects only future exits, as the README promises. Two
  windows showing the tab would both try to close it. Close assessment and
  commit run on workers, and their completions belong to the window.
- Reconcile removes views, orders tabs, and publishes titles but never
  installs a view (`windows/projection.rs`). It visits only windows whose
  workspace or tabs a batch touched, and attaching a session emits no
  hierarchy event.
- `prepare_close` already resolves `CloseRequest::Window` to `Detach` when
  another attachment holds the session, so closing one of two windows on a
  session keeps the session.

## Scope

In scope:

- A core viewer registration with capabilities, checked by the runtime on
  every request, including read-only rejection.
- Revisioned terminal status replacing `TerminalEvent` delivery.
- Per-viewer wakes, invalidation rearm, and a shared snapshot cache keyed by
  a publication revision.
- Shared viewport publication to every viewer, and returning to live output
  on input at the runtime.
- Focus aggregation, the controlling viewer, `latest` sizing, presentation
  authority, and mouse gesture ownership.
- Host-effect recipients owned by viewers and woken through them.
- Desktop migration to one viewer per `TerminalView`, then canonical grid
  display, installing views for tabs a window did not spawn, one exit-close
  actor, and a two-window smoke fixture.

Out of scope, with the issue that owns each:

| Deferred work | Owner |
| --- | --- |
| Smallest, largest, and manual sizing policies, their config and commands | [#207][issue-207] |
| Attachment events in the hierarchy stream | [#26][issue-26], [#21][issue-21] |
| A user-facing way to show one session in two windows | [#26][issue-26] and the workspace sidebar |
| Closing or retargeting a window emptied by another client | [#21][issue-21] |
| `close_on_exit` as a runtime policy shared by clients | [#45][issue-45] or [#21][issue-21] |
| Pane split trees, per-pane viewers, local zoom, and split-geometry tests | [#28][issue-28] |
| Wire codec, per-connection budgets, row deltas, and reconnect | [#44][issue-44] |
| Browser and TUI viewers and mixed-client host-effect tests | [#134][issue-134], [#48][issue-48] |

## Principles

The [hierarchy projection](hierarchy-projection.md) principles apply:
reactive and never polled, idle costs nothing, work scales with the change,
correctness never depends on observing a particular event, and loss is
detected. This plan adds:

- **The single-viewer path behaves as it does today.** One viewer is always
  the controller, shown or hidden, so hidden-tab resizing, exited-tab
  reflow, and theme reloads keep working. One viewer means one wake and one
  snapshot per frame allowance. The added publication cost per output chunk
  is a short, uncontended lock instead of one atomic exchange, and the
  benchmark gates must show it stays within budget.
- **The owner thread is the only arbiter.** Size, presentation, focus
  reports, and mouse ownership are decided on the terminal's runtime thread,
  in the order requests arrive. No client decides for another.
- **State over events.** A viewer reads current state when woken. A viewer
  that wakes late, or starts late, sees the same result as one that saw
  every change.
- **Viewer errors never harm the terminal.** Permission and revocation
  errors answer the requesting viewer only. They never pass through the
  runtime's fatal paths.
- **Accepted gestures always end.** No rule may discard the release of a
  button the application received a press for.

## Terms

Add to `CONTEXT.md`:

- **Viewer:** a client's registration to display and interact with one
  terminal, with its own capabilities. One attachment can hold many viewers,
  one per displayed terminal. A viewer is not an attachment and does not
  affect session membership.
- **Controlling viewer:** the viewer whose geometry and presentation the
  terminal uses.

`AttachmentId` keeps meaning a view's connection to a session.

## Decisions

### A viewer is a runtime registration distinct from an attachment

`Mux::subscribe_terminal(attachment, terminal, ViewerOptions)` validates
that the attachment is live and that the terminal belongs to its session,
as `register_host_effect_recipient` does today, and returns a
`TerminalViewer`. It replaces `register_host_effect_recipient`,
`register_presentation_controller`, `Mux::runtime_clients`, and the public
`Mux::attach`.

A `TerminalViewer` holds:

- a `ViewerId`, a scoped ID in `huterm-protocol` like `AttachmentId`, used
  for diagnostics and the future wire;
- a shared slot that every request carries, so the owner thread checks
  revocation and capabilities without a lookup, following
  `PresentationUpdate`;
- its own capacity-one wake;
- its host-effect recipient, when its options allow host effects.

`ViewerOptions` carries:

- the capabilities;
- the host-effect options (origin and clipboard permission);
- the viewer's initial focus, geometry, and presentation, any of which may
  be empty; initial focus needs `input` or `size`, like a focus report;
- `spawned`, set only by the client that created the terminal, which gives
  the viewer a running lifecycle baseline (see "Terminal status is
  revisioned state").

Registration inserts the slot into the terminal's registry under the same
short lock the owner thread takes to publish, and the slot starts notified
with its wake signalled. A viewer therefore cannot miss a publication that
lands between subscription and its first poll, and it reads its status
baseline after it is registered. The owner thread adds a viewer to its
arbitration table when it reconciles: at the top of each turn, and again
before a request it dequeues when the registry has changed since. A viewer
marks the registry when it registers, before it can queue anything, so
every request finds its viewer's entry. Viewer IDs
are numbered across the process, so no two terminals in one runtime scope
issue the same ID. Initial geometry is clamped like a reported one. A
viewer with host effects registers its recipient before its slot, so a
failed registration never leaves a slot whose initial focus and size the
owner thread could apply; a closed terminal answers `Stopped`.

A terminal admits at most 32 live viewers, the existing
`TERMINAL_RECIPIENT_LIMIT`; past that, subscription fails with a
`MuxError`. A slot stops counting toward the limit when the owner thread
finalizes it (see "Revocation and lifecycle").

`RuntimeClient` becomes crate-private. Close tickets keep using it for job
assessment, which is runtime-internal. `OpenedTab` stops returning a client:
the spawning window subscribes under the same Mux lock. Standalone
`TerminalRuntime` users get `TerminalRuntime::subscribe(ViewerOptions)`; the
caller owns that runtime, so it is the authority. The migration covers GPUI
scroll tests, benchmarks, `crates/huterm-core/tests/descriptor_limits.rs`,
and `crates/huterm-gpui/examples/native_quit_smoke.rs`.

When the runtime stops, including by a panic on its thread, it closes every
viewer's wake explicitly, so a viewer's wait ends with `Stopped` even while
its handle, the registry, or the host-effect sink still holds the channel.
Finalizing a revoked viewer closes its wake the same way, after the
revocation signal, so its activity task ends.

### The runtime checks capabilities on every request

`ViewerCapabilities` in `huterm-protocol` has independent flags:

| Flag | Allows |
| --- | --- |
| `input` | Keys, characters, text, paste, mouse reports, clear scrollback, and reset; its focus counts toward application focus reports |
| `viewport` | Scroll commands on the shared viewport; with `input`, typing returns the viewport to live |
| `size` | Geometry reports; its focus, typing, and geometry can make it the controller |
| `host_effects` | Registration as a host-effect recipient |

`set_focus` is accepted from a viewer with `input` or `size`, and each flag
decides which effect the report has. Every viewer can request snapshots,
selection text, and link lookups, and can submit a presentation, which
applies only while it controls. A read-only viewer lacks `input`. With
`size` it can still take control by focus. A viewer with `input` but not
`size`, like a tmux client that ignores size, types and reports focus but
never controls size. A CLI connection that only reads snapshots holds no
flags, so it never controls the viewport or size.

The handle rejects a request it lacks the capability for with a new
`RuntimeError::NotPermitted`, before reserving input bytes or queueing. The
owner thread checks again at dequeue, in one place for every ordered request:
it discards input, edits, and arbitration reports from a revoked viewer or one
whose capabilities do not permit them, releasing their byte reservation, and
answers a refused scroll with `NotPermitted`. In process, capabilities are
immutable, so the handle's check normally decides and the dequeue check
matters for revocation. Over [#44][issue-44] the server repeats the capability
check per message, so enforcement does not depend on client code.

`NotPermitted` and `Revoked` are viewer errors. Snapshot and selection
controls from a revoked viewer are answered with `Revoked` through their
reply channel, never through `complete_snapshot_request`'s fatal path and
never by dropping the reply sender, which the client would read as
`Stopped`.

### Terminal status is revisioned state

A `TerminalStatus` in `huterm-protocol` replaces `TerminalEvent`:

- `revision`: advances only when a published value changes;
- `title`;
- `metadata` and its existing metadata revision;
- `lifecycle`: running, or exited with its `ExitStatus`;
- `bells`: a count of bells since the terminal started;
- `failures`: the most recent failure messages, at most 8, each with a
  sequence number.

The owner thread compares before publishing. A program that re-sends an
unchanged title on every chunk, as the Ghostty adapter reports today,
changes nothing and wakes nobody. A real change replaces an
`Arc<TerminalStatus>` behind a short lock and wakes every viewer. A viewer
reads the current `Arc` when its revision differs from the last one it read.
Floods cost constant memory.

A viewer derives what changed by comparing with a baseline:

- The exit transition is a lifecycle change from running to exited since
  the baseline. A viewer subscribed with `spawned` takes a running baseline,
  so a command that exits before its window subscribes still produces the
  exit transition. Any other viewer takes the status it first reads as its
  baseline and reports no transition for an exit that happened earlier.
- New bells are the count minus the baseline count. A late viewer does not
  ring for old bells.
- A gap in failure sequence numbers reports dropped failures instead of
  hiding them, keeping loss visible.

`Ready` has no replacement: a viewer exists only for a published, started
terminal. The runtime stopping, by close or fatal failure, closes the wake;
the final status stays readable after closure, so a viewer finishes its
last update before retiring. A revoked viewer's poll reports revocation.

The content generation does not belong in the status. Output changes it on
every chunk, and status changes wake every viewer, so including it would wake
hidden viewers for every chunk. The generation travels with invalidations
instead.

### Publication wakes each viewer once per snapshot

The runtime keeps a **publication revision**. Every change that can alter a
snapshot advances it through one function, the successor of
`publish_invalidation`: output, buffer edits, applied resizes and
presentations, including those the controller decides, and viewport moves.
Each viewer slot has its own `notified` flag and its own
earliest-invalidation instant:

1. On a publication, the owner thread sets each clear `notified` flag,
   records the instant, and signals that viewer's wake. A viewer that is
   already notified gets no further wake.
2. When the owner thread builds a snapshot for a viewer, it clears that
   viewer's flag and takes its instant. This keeps the existing rule:
   rearm on the owner thread at snapshot construction, not when a client
   drains its notification, so hidden or frame-blocked viewers do not wake
   for every output chunk.

A viewer's poll returns the latest status if it changed, whether it is
notified along with the current content generation, new bells, new
failures, and revocation. The desktop clears a stale selection from that
generation as it does from `Invalidated` today.

### Viewers share one snapshot per revision

`SnapshotReply.snapshot` becomes `Arc<TerminalSnapshot>`, and
`TerminalSnapshot` gains its publication `revision` and its **geometry
revision**, which advances on every canonical resize. The owner thread keeps
the last snapshot and the revision it reflects. A request without a scroll,
or with a scroll that leaves the viewport where it is, at the same revision
reuses that `Arc` instead of rebuilding. Link lookups still run per request
against the reused snapshot.

The cache is safe only if every snapshot-affecting change advances the
revision, so all of them go through the one publication function above.
Perturbation tests cover the two new paths: viewport moves and
controller-driven resizes (see Verification).

Under continuous output, two visible viewers usually request at different
revisions, so each builds its own snapshot. The cache pays off for static
content, scrolling, and bursts that both viewers see at one revision.

### The shared viewport publishes to every viewer

An admitted scroll changes the runtime viewport for everyone, as today. When
the scroll actually moves the viewport, the owner thread publishes, notifies
every other viewer, then builds the requesting viewer's snapshot and clears
its flag. The requester is not woken by its own scroll.

Returning to live output on typing moves into the runtime, so a view that
has not yet seen another viewer's scroll cannot leave the shared viewport
in history after typing:

- When a `Key`, `Character`, `Text`, or `Paste` input from a viewer with
  `input` and `viewport` is dequeued while the shared viewport is above
  live, the runtime moves it to live and publishes. The check reads the
  engine's viewport, a constant-time query.
- Each viewer numbers its own scroll commands, and every input carries the
  number of that viewer's latest scroll when the user typed it. If the same
  viewer has a later scroll already applied, the input does not return to
  live. Scroll controls overtake queued input on the owner thread, so
  without this rule a key still queued behind output would undo a scroll the
  user made after typing. The comparison uses only numbers the viewer
  assigned itself, so it works unchanged when the viewer is a remote client.
  A race between one viewer's typing and another viewer's scroll resolves in
  runtime order.
- The opposite order needs the same rule. A scroll that arrives after a turn
  drains its controls can be dequeued after a key the user typed later, so
  typing that returns to live records the scroll number it carries, and a
  later-applied scroll from that viewer numbered at or below it does not
  move the viewport. Its snapshot still answers. Both scroll numbers live
  on the viewer's slot, which every request carries, so they hold before
  the owner thread first reconciles the viewer and after it finalizes it.
- On typing, the view clears its own pending scroll intent, desired offset,
  and wheel remainder, as `ScrollController::bottom` does today, but submits
  no `Live` command. An older snapshot that completes afterwards then has no
  pending intent to replay, and a scroll made after typing is kept.
- The runtime move requests no snapshot, so keystroke echo keeps its
  one-snapshot pacing.

Selection stays valid because buffer points are bottom-relative and the
content generation ignores scrolling. Each viewer's `ScrollController`
clamps pending relative intent against authoritative completions, so
concurrent scrolls converge in runtime order. A selection drag that
autoscrolls one viewer moves the shared viewport for all viewers; that is
the intended meaning of a shared viewport.

One limitation is accepted: a wheel scroll in a view whose display is one
snapshot behind clamps against stale bounds. Wheeling toward live in a view
that still shows live, after another viewer scrolled into history, sends
nothing. The next snapshot, at most one frame later, shows the moved
viewport, and the user's next wheel event applies.

### Arbitration reports bypass input admission but keep its order

A viewer reports two kinds of arbitration state: focus and geometry
(`GridSize` and `CellSize`). These reports differ from input:

- **Geometry is latest-wins.** It keeps today's `pending_resize` path: the
  view holds the newest geometry and retries it on `Busy`. A window resize
  during a paste sends its final geometry once instead of filling the queue.
  Geometry may overtake queued input, as a resize does today; a newer
  geometry crossing a queued key only resizes sooner.
- **Focus changes are ordered entries in the `InputQueue`.** The
  application must see focus in, the keys typed while focused, and focus
  out in that order, and blur enqueues its mouse releases before focus out.
  A focus change replaces the pending one only when no input was queued
  between them, so the queue holds at most one focus entry per queued input
  entry, plus one. The queue retries in order and stops at the first
  `Busy` refusal, so a refused focus change is a barrier: no later key can
  overtake it.
- **Neither is refused for capacity or closed by exit.** Focus entries sit
  outside the `InputQueue`'s event and byte limits and keep flowing after
  root exit, and geometry never entered the queue. The runtime processes
  both after exit too, as it resizes the emulator today, so an exited tab's
  retained history keeps reflowing and its viewer keeps taking part in
  control.

On the runtime, arbitration reports are their own `RuntimeMessage` kind on
the ordered `messages` queue.

A hidden view keeps reporting geometry changes from `sync_tab_layout`, so a
background terminal still follows font, padding, scale, and bounds changes,
as today. Visibility plays no part in control (see below), so views report
no visibility. The follow-up sizing issue can add a visibility report if a
policy like tmux's `aggressive-resize` needs one.

Focus keeps today's source: the view's `on_focus` and `on_blur`, which
already include window activation.

### Focus reports are aggregated

The owner thread counts focused viewers that have `input` and writes
`ESC [ I` only when the count rises from zero and `ESC [ O` only when it
falls to zero, under the existing `focus_reporting` mode check. A dropped
or revoked viewer that was focused counts as focus out when it is
finalized. `TerminalInput::Focus` is removed.

With one viewer this writes exactly what Huterm writes today. Moving between
two Huterm windows on one terminal can write focus out and then focus in,
when the first window's blur arrives before the second window's focus; that
matches the order the user moved focus in.

### The controlling viewer owns size and presentation

Each viewer slot records, on the owner thread, its geometry, focus, latest
submitted presentation, and an activity ordinal. The ordinal
advances when the viewer gains focus or sends a `Key`, `Character`, `Text`,
or `Paste` input, including after root exit, when the input itself is
discarded. A repeated focus gain counts too: a view's queue coalesces a
blur and the refocus after it into one gain, and that refocus must still
take control back. Mouse reports, scrolling, focus out, and synthetic
releases do not count. Mouse presses are excluded because a press that moved
control would resize the grid that the press's own coordinates were
computed against; on the desktop the click that activates a window already
transfers control through focus.

The controlling viewer is chosen from candidates: viewers with the `size`
capability that have reported geometry and are neither revoked nor dropped.
The candidate with the highest activity ordinal controls, ties going to the
most recent registration. Whether a viewer is currently shown does not
matter. This matches tmux's `latest`, which follows the client with the
most recent activity on a window among every client whose session contains
it, shown or not; only its `aggressive-resize` option restricts that to
clients currently showing the window. Switching a window's tab away from a
shared terminal and back therefore keeps that window in control and
resizes nothing.

A single viewer is always the controller, which preserves today's
hidden-tab and exited-tab behavior. A dropped viewer stops being a
candidate as soon as it is dropped, even while its queued input still runs,
so closing a window does not resize the PTY back to that window's geometry
first. The owner thread recomputes the controller after an arbitration
report, an activity change, a drop, or a finalization. When the controller
or its state changes:

- If the controller's `(GridSize, CellSize)` differs from the canonical
  size, the runtime resizes the PTY and the engine, advances the geometry
  revision, and publishes. After root exit it resizes only the engine, as
  today, and drops any reply the resize produces, such as an in-band size
  report, because the writer has closed.
- If the controller's presentation differs from the applied one, the
  runtime applies it and publishes.

With no candidate at all, size and presentation stay as they are. Input that
changes the controller resizes the terminal before that input is written, so
the application sees the new size first. The canonical `CellSize` comes from
the controller too, so the PTY's pixel size and geometry query replies follow
the controlling viewer's display scale.

Keep the selection in one pure function from viewer slots to the outcome.
The follow-up sizing issue adds policies there without touching the
protocol.

This replaces `PresentationController`, `PresentationAuthority`, and the
public `RuntimeClient::resize`. A viewer's presentation update never fails
because another viewer exists.

Consequences to document:

- Moving between two windows of different sizes on one terminal resizes the
  PTY each time, as tmux's `latest` does.
- A quake window and a regular window on one terminal at different sizes
  resize the PTY whenever focus moves between them: summoning the quake
  window focuses it and takes control, and it keeps control while hidden
  until the regular window regains focus or receives input.
- A view that shows the terminal while another, hidden view controls it
  displays the canonical grid clipped or with background until the user
  focuses or types in it.

### Mouse gestures belong to the viewer that pressed first

A `MouseArbiter` on the owner thread holds the policy:

- A press from a viewer with `input` starts or extends that viewer's
  gesture.
- While a gesture holds buttons, every mouse report from other viewers is
  discarded: presses, releases, motion, and wheel reports.
- Releases from the owner end its buttons, matched by button only. The
  gesture ends when none remain. A release with no matching held button is
  discarded.
- Hover motion and wheel reports with no gesture held are accepted from any
  viewer with `input`. Motion that names a held button is accepted only from
  the owner, so a drag whose press was refused, or whose gesture has ended,
  never reaches the application without its press. (Decided before PR B;
  PR 1 accepted it from any viewer when no gesture was held.)
- Every input carries an `InputStamp` recorded when the user acted. Its
  optional geometry revision names the snapshot that mouse coordinates came
  from. If the canonical geometry changed since, presses, motion with no
  gesture held, and wheel reports are discarded, so a resize between
  computing a report and dequeuing it cannot move it to another cell. A
  revision, unlike a grid size, also catches a resize there and back. In
  PR 1 the desktop computes mouse coordinates from its own layout and sends
  no geometry revision, so this check never runs; PR A stamps reports once
  it maps them against the displayed grid.
- Motion and releases inside an owned gesture are never discarded for
  geometry; the encoder clamps their coordinates to the current grid, as it
  does today. Accepted gestures therefore always end.
- When the owner viewer is finalized with buttons held, the runtime writes
  releases for those buttons at the owner's last reported position. Only
  the owner's reports move that position.
- When the application turns mouse tracking off during a gesture, the buttons
  held then stop blocking other viewers. Their releases are still the owner's,
  including after the owner presses again, another viewer's next admitted press
  abandons them, and the owner pressing one of those buttons again retires its
  earlier press; a refused press, such as one with a stale geometry revision,
  leaves them in place. A view that saw tracking stop never sends their
  releases, so other viewers' presses would otherwise stay discarded once the
  application tracks the mouse again. A view that did not see it, because
  tracking came back on before its next snapshot, still sends them, and the
  runtime still writes them. A report dequeued while tracking is off, such as a
  press computed against an older display, writes nothing and takes no gesture.

A view whose press was discarded keeps its local ownership state until its
own release and gets no feedback; its drag motion and release are discarded
with it. That is acceptable for the default. The
arbiter runs before encoding, on structured `TerminalInput::Mouse` values.
Experimenting with another rule changes only this type.

### Host effects wake their own viewer

A viewer created with `host_effects` registers its recipient during
`subscribe_terminal`. Admission signals that viewer's wake instead of the
terminal's activity channel, so the selected window always wakes.

Recipient selection keeps today's eligibility, generations, budgets, and
no-redirect behavior, and orders recipients like the controller: highest
activity ordinal first, ties going to the most recent registration. A
hidden recipient stays eligible, so a background tab's OSC 52 write still
reaches the clipboard, as it does today.

### Revocation and lifecycle

A viewer ends in one of two ways. In both, the owner thread **finalizes**
it once: it removes the viewer from arbitration, counts focus out, releases
its mouse gesture, recomputes the controller, and frees its slot.
Finalization is idempotent, so a viewer that is dropped and revoked at once
is finalized only once.

- **Revocation** withdraws authority. Mux revokes viewers wherever it revokes
  host-effect recipients today: `detach_session`, `retarget_attachment`,
  `close_session`, and cross-session `move_tab` and `move_workspace`. Terminal
  close and `shutdown` stop the runtime instead, so its viewers' requests return
  `Stopped`. Revocation marks the slot, flags the registry as changed, and wakes
  the owner thread, so an idle owner thread finalizes at once. Queued requests
  from the viewer are discarded at dequeue, and its requests return `Revoked`.
- **Dropping** a viewer marks the slot dropped, flags the registry as
  changed, and wakes the owner thread. It never blocks and never takes the
  Mux lock, so it is safe from `on_app_quit`. The viewer stops being a control
  candidate immediately. Its queued input, buffer edits, and arbitration
  reports were admitted with valid authority, so they still run in order;
  a focus change queued before input still reaches the application before
  that input, but no report makes the viewer a candidate again. The slot
  counts its queued input, edit, arbitration, and presentation messages,
  incrementing before each send and decrementing when a send is refused or
  a message is processed, and the owner thread finalizes a dropped viewer
  once that count reaches zero. A press queued before the drop is therefore
  written before the synthetic release, never after, and a focus handoff
  queued before a window closes writes no spurious focus out and in.

While the application stops reading its PTY, the owner thread dequeues no
messages and does not reconcile its viewer table, so a dropped viewer's
finalization and the release of its slot wait for that backpressure to
clear. The release bytes would wait behind the same backpressure anyway. A
full 32-viewer limit during such a stall refuses new viewers instead of
growing, which bounds the focus reports and releases reconciliation can
queue: if finalization freed slots during the stall, subscribe-and-drop
churn could queue them without limit. Once the writes drain, the owner
thread reconciles before it handles another request or waits. The
notification that asked for the reconcile was consumed during the backlog,
so nothing else is guaranteed to wake it, and a request must not run ahead
of a finalization it follows: a press handled before a dropped gesture
owner is finalized would be refused. The reconcile before a request runs
as part of handling that request. A pending change never sends the turn
back to its start, because viewers registering and dropping in a loop
would then keep the owner thread from ever reaching a request or PTY
output.

A viewer never keeps its runtime alive. Teardown order, child reaping, and
bounded shutdown are unchanged. Close tickets use the crate-private client,
so assessment works without a viewer.

### Desktop: one viewer per terminal view (PR 1)

`TerminalView` replaces `client`, `presentation`, and `host_effects` with
one `TerminalViewer`. The tab activity task waits on the viewer's wake. With
one viewer, which is always the controller, behavior stays as it is:

- `refresh` polls the viewer instead of draining 64 events. Host effects
  keep their 8-per-turn budget.
- `resize_to_layout` reports geometry through the arbitration rules above,
  for hidden tabs as well. `pending_resize` becomes the general
  pending-arbitration state.
- `on_focus` and `on_blur` report focus through the same rules. The
  existing test that releases precede focus out moves to the new reports.
- `publish_presentation` submits through the viewer.
- Typing clears local scroll intent and no longer submits `Live`; the
  runtime returns to live.
- `Revoked` shows no notice. The view stops sending. PR B's reconcile
  removes a view whose tab left the window's session and replaces the
  viewer of one whose tab stayed.

PR 1 intends no user-visible behavior change.

### Desktop: the displayed grid follows the canonical size (PR A)

A view works with three grids:

- The **requested grid** is the one `TerminalLayout` computes from the
  view's bounds, as today. The view reports it to the runtime, which uses it
  only while this viewer controls.
- The **displayed grid** is the `size` of the snapshot the view shows: the
  canonical grid when that snapshot was built. It differs from the requested
  grid while another viewer controls, and for one snapshot after this view's
  own layout changes.
- The **visible area** is the part of the displayed grid the view can show:
  the pixel rectangle where the displayed grid's extent, at this view's
  cell size, overlaps the requested grid's painted bounds. It is measured
  in pixels, not cells, because `TerminalLayout` requests at least one cell
  even when less than one fits. A viewport narrower than a cell then paints
  part of that cell, and today's hit test accepts the painted part. Only
  bounds that padding reduces to nothing have an empty visible area.

Until PR A, `TerminalView::last_grid_size` is both the requested grid and
the grid the view assumes it displays. PR A renames it `requested_grid` and
reads the displayed grid from the snapshot. Before the first snapshot
nothing is displayed: hit tests answer "outside", and the page size,
scrollbar, and size panel fall back to the requested grid.

Painting needs no new code:

- `TerminalRenderer::paint` already paints the snapshot's own `size` from
  the layout origin, and `render` already clips to the requested grid's
  cell-aligned bounds. A larger canonical grid is therefore cut at the last
  whole cell the view requested, and a smaller one leaves the view's
  background to its right and below. The clip does not widen to the padded
  available area: that would draw a partial column and row, and it would
  change what a single viewer shows while a shrinking window waits for its
  snapshot, which the desktop input guide requires to stay clipped.
- The origin stays the layout origin, including the `padding_balance`
  offset computed from the requested width, so nothing moves while the two
  grids agree. A smaller displayed grid is not centred again.
- Rows and columns beyond the clip are still prepared and painted. Their
  cost is bounded by the controlling view's grid, so PR A adds no culling.

One pure type, `GridGeometry`, holds the requested grid's painted bounds,
the cell size, the requested grid, and the displayed grid. It answers
whether a position is inside the visible area, which displayed-grid cell a
position maps to, where a cell is if any of it is visible, how many rows a
page scrolls, and which way a selection drag past an edge scrolls. It lives
in its own module so hit testing is unit-tested without a window and panes
([#28][issue-28]) can reuse it. Cell coordinates are always in the displayed
grid and clamp to its nearest edge; only the inside test and edge detection
use the visible area.

| Consumer | Grid it uses |
| --- | --- |
| Application mouse reports and link points | Inside test against the visible area; coordinates in the displayed grid |
| Selection points | The displayed grid, as `point_for_position` already clamps |
| Selection edge scrolling | The visible area's height to detect the edge; the displayed grid's first and last rows for the head, as today |
| Page scrolling, from commands, a scrollbar track press, and the scroll benchmark | The smaller of the requested and displayed rows |
| Scrollbar geometry | The displayed rows, because history and offset come from the same snapshot |
| Size panel | The displayed grid |
| Keyboard context-menu anchor and the IME candidate rectangle (`bounds_for_range`) | The cursor cell when it is inside the visible area, otherwise the grid's top-left corner |
| Geometry reports, `pending_resize`, and the reported pixel size | The requested grid |

The page size is the smaller of the requested and displayed row counts, not
the displayed rows. They are equal unless the canonical grid is taller than
the view, and there a page of displayed rows would skip the rows the view
cannot show.

Mouse reports:

- A new press, hover motion, or wheel report outside the visible area sends
  no mouse report. Motion and the release of a gesture the view already owns
  are sent wherever the pointer is, with coordinates clamped to the
  displayed grid.
- Outside the visible area the pointer gets the local behavior the padding
  has today: a wheel event scrolls the shared viewport, a left press starts
  a selection at the clamped point, and a right press runs the configured
  action. The background beside a smaller canonical grid is therefore not
  inert.
- Every `TerminalInput::Mouse` carries the geometry revision of the snapshot
  its coordinates came from, set in `admit_input`. Other input carries none.
  The queue already keeps the newer stamp when it coalesces motion, and a
  retried report keeps the stamp it was queued with.

Two kinds of change move the grid under a stationary pointer, and each needs
one small reconciliation:

- **A snapshot with a new geometry revision**, whichever viewer caused the
  resize. `apply_snapshot` compares the revision, not the size, and calls
  `MouseState::boundary`, which forgets the motion deduplication sample
  and the wheel remainder of less than one step. Otherwise a hover report
  that the runtime discarded as stale stays the sample and suppresses the
  next motion in the same cell. Queued input and gesture ownership are
  untouched, so a drag in progress continues.
- **This view's own layout change.** `resize_to_layout` already invalidates
  link state, and a snapshot follows only when this view controls. PR A
  also forgets the hover point there, so the next render maps the pointer
  again and requests a lookup even when no snapshot follows.

Link state needs nothing on a new geometry revision. A link result arrives
in the same reply as its snapshot and answers a cell, and another viewer's
resize moves neither this view's origin nor its cell size, so the pointer
maps to the same cell. Whether that cell is still inside the visible area
is decided again by the next render, which the changed grid size already
causes. Invalidating there would discard a result that is correct.

The content generation already clears the selection. The size panel is
raised only by this view's own viewport changes, so moving focus between two
windows does not flash it in both. A hidden view requests no snapshots, so
its displayed grid stays as it was when it was hidden, or empty if it was
never shown, until it becomes visible again.

A view that displays a canonical grid larger than its own accepts two
limits. It shows the top-left cells, so the bottom rows, usually the prompt,
are cut off until the user focuses or types in it and takes control. A
selection dragged past the clipped edge keeps extending into rows and
columns the view cannot show.

With one viewer, the displayed grid trails the requested grid for one
snapshot after each layout change: the view reports its geometry, the
runtime resizes and publishes, and the view shows the old snapshot until the
new one arrives, normally within a frame. This interval is the only
user-visible change PR A intends for one viewer, and every consumer in the
table above follows the displayed grid through it:

- The size panel, scrollbar thumb, page size, and context-menu anchor show
  the old grid for that snapshot and then the new one. A snapshot whose
  size differs always redraws the view, so they settle without a further
  event.
- After the view grows, the cells it gained are outside the visible area
  until the snapshot arrives. A press there starts a local selection
  instead of a report, as a press on the padding does.
- Today a press, hover motion, or wheel report in that interval is mapped
  against the requested grid, which the user does not see yet, and sent
  unstamped. PR A maps it against the displayed grid and stamps it. If the
  runtime has applied the resize by the time it dequeues the report, it
  discards the report as stale. If the report is dequeued first, for
  example while the geometry report waits in `pending_resize` behind a busy
  runtime, it is delivered against the grid the runtime still has, where
  today's code sends coordinates for a grid the runtime has not reached.
- The view cannot tell this interval from another viewer controlling, so it
  has no special case for it.
- A press discarded this way leaves the view owning the button locally, as
  PR 1 accepted for refused presses. Its release is discarded too. Until
  PR B, with drag tracking the application could see that drag's motion
  without a press, because the arbiter admitted held-button motion from a
  viewer that owned no gesture; PR B's first commit discards it.

Smoke state keeps `grid=`, and the integration smoke's `columns=` and
`rows=`, as the requested grid, so existing assertions keep meaning the size
this view asks the PTY for. Each writer adds `displayed_grid=`, or
`displayed_columns=` and `displayed_rows=`, reporting `0,0` before the first
snapshot. A hidden tab's displayed grid can be stale, so a check may compare
the two grids only for a visible tab. There are five state writers: the
palette, integration, presentation query, fullscreen, and quake smokes. The
refresh smoke only reads the field to build a pending resize, and reads
`requested_grid` instead. The presentation query smoke's `layout_grid`
stays.

Each view still renders with its own font metrics and scale. Only the cell
count is shared.

### Desktop: a GPUI test harness (PR A, extended in PR B)

`huterm-gpui` had no test that drives a view through GPUI; its tests were
state machines with fakes. The vendored `gpui-pre` has a `test-support`
feature with `#[gpui::test]`, `TestAppContext`, `VisualTestContext`
(simulated mouse, keystrokes, resize, scale change, frame requests, and
window activation), and a deterministic scheduler whose `allow_parking`
lets a test wait on real threads such as a terminal runtime.

The feature is enabled through an opt-in `test-support` feature on
`huterm-gpui`, not through a dev-dependency. Cargo builds examples with
dev-dependencies, and the smoke and benchmark binaries are examples, so a
dev-dependency would compile GPUI's leak detection and debug-bounds records
into them. The harness and its tests compile only under both `test` and the
feature, and no production code is conditional on it, so one Clippy pass
with the feature lints everything the pass without it did. The Cargo
commands in the `lint`, `typecheck`, and `test:rust` tasks pass
`--features huterm-gpui/test-support`; `check`, `check:rust`, `ci:lint`,
`ci:test`, and the commit hook reach those three. Those `--all-targets`
commands also compile the examples with the feature, but the smoke and
benchmark tasks build their examples in their own Cargo commands without
it. A bare `cargo test -p huterm-gpui` skips the harness tests; the
development guide says so. A task that loses the flag would skip them
without failing, so a script test, in the style of
`scripts/policy-alignment.test.ts`, asserts that the three tasks pass it.

The feature adds `proptest` and its dependencies to `Cargo.lock`, which
needs `mise run license` and the release-age policy. On Linux it also sets
GPUI's `wayland` cfg, which the workspace otherwise leaves off; the spike
result below records that the Linux test build needs no extra packages.

Tier 1, in PR A, is one `TerminalView` as the root of a test window over a
standalone `TerminalRuntime`:

- The helper spawns a `/bin/sh` fixture, subscribes a viewer, opens a test
  window of a chosen size, builds the view with fixed `GridMetrics`, marks
  it visible, and activates and focuses it. `GridMetrics` has private
  fields, so `renderer.rs` gains a test-only constructor. Activation is
  `Window::activate_window`.
- A test window starts at scale factor 2.0 and the view rescales its
  metrics on its first layout. Tests compute pointer positions from the
  view's metrics after the first draw, never from the values they passed
  in.
- The view is configured without chrome: hidden chrome and no tab bar, so
  its content origin is zero. `WorkspaceView` normally places the view at
  that origin; as a window root the view paints at the window's origin, so
  any other content origin would make painting and pointer mapping
  disagree. The helper asserts the zero origin.
- The helper draws the view before any input. GPUI raises focus callbacks
  from drawn frames and the view mounts its pointer listeners while
  painting, so a view that was never drawn receives neither. Before a test
  goes on, the helper waits until the runtime has the view's focus report
  and the view shows a snapshot with the mouse tracking mode the fixture
  enabled.
- It runs a drain shaped like the tab activity task: wait on the viewer's
  wake, then call `refresh`. Snapshot replies arrive on the view's own
  tasks.
- Frames come from `Window::simulate_next_frame`, the only public frame
  driver; `TestWindow` and its frame request are private to GPUI. GPUI's
  test mode draws dirty windows but never runs next-frame callbacks, and
  `SnapshotPacer` regains its allowance only inside one. Without a pumped
  frame the view takes one snapshot and then stops, with no further wake or
  notification.
- Each wait is a loop: run the drain's pass, pump a frame while the view's
  frame clock has callbacks pending, and check the condition, under a
  bounded deadline that reports the last observed state. The deadline is
  shorter than GPUI's 15-second parking limit. Waits never sleep for a
  fixed time.
- If the frame clock cannot be driven that way, the harness sets
  `refresh = "unlimited"`, which only removes the per-frame snapshot
  allowance. It does not replace drawing, and the spike test stays on
  normal frame pacing if it can.
- A second viewer needs no second window. The test holds a bare
  `TerminalViewer` on the same runtime and makes it the controller with a
  focus report sent after the view's own, so the view's displayed grid
  differs from its requested grid for as long as the test needs.

Tier 1 has known limits. Font metrics are synthetic. A test window has no
native handle. Without the production `WorkspaceView` root,
`windows::modal_focus` and `windows::modal_showing` answer "no dialog", and
on macOS `windows::terminal_input_allowed` refuses text through the input
handler. Tier 1 therefore covers pointer input, focus, geometry, and
refresh, and leaves typed text to tier 2.

Tier 2, in PR B, hosts `Desktop` and `WorkspaceView` with two windows on one
session, which reconcile, exit-close, and replacement tests need.

Tier 2 result, PR B part 1: it works on macOS. `install_desktop`, split out
of `run_with_startup`, installs the production `Desktop` global in a test
app, and `open_window_inner` opens real `WorkspaceView` windows on GPUI's
test platform. The harness builds a session and its tabs through Mux and
attaches each window to it, so every view is installed rather than spawned;
the spawn path stays covered by the smokes. Typed keys reach a terminal only
once its window reports itself active and the terminal is focused, which
the harness waits for after activating a window. Each test takes under one
second, and the leak detector passes with the windows and runtimes still
open at the end.

The harness is the first implementation step of PR A, as a spike. PR B's
test plan depends on its answer, so if a real `TerminalView` cannot be built
and driven in a test window, the work stops and the maintainer decides
before falling back to pure functions and smokes. The spike's result is
recorded here, with an answer to each of these, which reading the source
could not settle:

- whether GPUI's leak detector, which panics on a surviving entity handle,
  passes when the test drops the view and its runtime;
- whether `allow_parking` before the first cross-thread wake is enough for
  the scheduler to accept the runtime's threads;
- what the drain's and the retry's timers cost while the scheduler parks,
  which it does in 100 ms steps;
- whether `TerminalRenderer::prepare` shapes and paints with the test text
  system;
- whether the Linux test build compiles and links with GPUI's `wayland`
  cfg set, and which packages it needs.

Spike result, 2026-10-15: the tier 1 harness works on macOS and in the Linux
container, so PR B builds on it.

- A real `TerminalView` in a test window receives simulated pointer events,
  and a click reaches the child as one press and one release. The test
  passed 30 runs in a row, and moving the click to another cell fails it at
  its assertion.
- The leak detector passes when the test drops the view and its runtime.
- `allow_parking` before the runtime starts is enough. Parked time advances
  the test clock, so the drain's and the retry's timers run normally; each
  tier 1 test takes under 1.5 seconds.
- `TerminalRenderer` prepares and paints with the test text system.
- The Linux test build compiles and passes with the `wayland` cfg set and
  needs no packages beyond the container's.
- One limit the source did not show: a simulated window resize moves GPUI's
  test pointer to the window's origin, because `Window::bounds_changed` reads
  the test platform's pointer, which never moves. A test that needs a still
  pointer across a layout change changes the layout through the view, as the
  hovered-link test does with a bottom tab bar.

### Desktop: installing, replacing, and removing views (PR B)

Reconcile gains one more derived output: projected tabs in the window's
workspace that have no installed view and no install in flight. It skips a
window that is busy, has no attachment, or is terminating. A window's own
spawn sets `busy` until it pushes its view, so reconcile never installs a
tab that the window is already spawning.

The window installs those tabs as follows:

1. A worker locks Mux, calls `subscribe_terminal` for each tab with the
   window's attachment, and returns the viewers with `hierarchy_seq()`.
2. The completion waits for the projection to reach that sequence, as other
   completions do.
3. In a deferred window update, it pushes a `TerminalView` for each tab that
   the projection still holds in the window's workspace without a view and
   whose viewer is not revoked, and drops the other viewers. It applies the
   installed order and selects a tab only when the window has no active
   tab, without taking focus from an overlay. That tab is the window's
   first in installed order.

Core offers no way to read a viewer's revocation without polling it, and a
poll would consume the status the view's first refresh needs. The window
therefore builds the `TerminalView`, whose constructor already submits the
initial presentation, and drops it when that request answers `Revoked`.

A subscription that fails because its target is stale is dropped silently
and not retried until a later batch touches that workspace, so a failure
cannot loop. Other failures wait for such a batch too. Once the runtime is
terminating, install failures show nothing; before that, unexpected errors
show a notice. Closing the window drops the in-flight task and the viewers
it returns.

When a view's viewer reports revocation and the projection still holds its
tab in the window's workspace, the window re-subscribes **in place**: the
same worker and sequence wait produce a new viewer, which replaces the old
one inside the existing `TerminalView`. Selection, scroll state, and the
active tab are kept; the selection is kept only while the content
generation is unchanged (see below). The new viewer is created with the
view's current
focus, geometry, and presentation as its initial state, because
GPUI raises no new focus event for a view that stays focused; otherwise the
old viewer's finalization would leave the application believing the
terminal lost focus. The tab's activity task waits on its viewer's wake, so
installing the replacement cancels that task and starts a new one on the
new viewer; otherwise output, status, and host effects for the replacement
would have no consumer. Input still waiting in the view's `InputQueue` is
discarded, since the old viewer's authority ended. If re-subscription fails,
the view is removed.

Decided before PR B: issue #206's "revoking a viewer keeps the selection"
holds while the content generation is unchanged. Replacing the controlling
viewer beside a viewer of another size hands control to that viewer and
back, so the terminal resizes twice; each resize advances the generation,
and the view clears its selection as it does for any resize. Avoiding that
would need a control handoff in core, which nothing needs until replacement
has a production trigger ([#21][issue-21], [#26][issue-26]).

Removal gains one case: a tab whose workspace now belongs to a session other
than the window's attachment session is removed, as a tab moved to another
workspace is. What the window does once emptied belongs to
[#21][issue-21].

A window that gains an attachment, including the smoke fixture's, runs its
own full reconcile. Attaching emits no hierarchy event, so nothing else
would visit it.

Found in PR B part 1: a view installed after its tab exited never learned
that it had exited, because the refresh reacted only to the viewer's exit
transition, so its tab lacked the exited state, label, and key context. The
refresh now also takes the exited lifecycle from status. Only the
transition is reported to the window, so such a view still announces
nothing and queues no exit-close.

Built in PR B part 3, on 2026-10-22:

- Reconcile derives replacements beside installs (`projection::replacements`):
  installed views whose viewer reported revocation and whose tab the
  projection still holds in the window's workspace, skipping a busy or
  terminating window. They share `PendingInstalls` with installs, so a
  replacement in flight is not started twice. A revocation emits no
  hierarchy event, so the tab's activity drain runs `reconcile_own` when it
  sees its view revoked.
- `subscribe_tabs` takes each tab's starting state. Installs start with no
  geometry; a replacement starts with the view's requested grid and cell
  size and its presentation. Every desktop viewer starts unfocused. The
  completion swaps the viewer into the same `TerminalView` only while the
  view is still revoked and the tab still belongs to the window.
- Changed in review on 2026-10-22: the state a replacement started with
  can be stale by the time it is swapped in, because focus, size, and theme
  can change during the worker and the sequence wait. The view records
  them even though its revoked viewer refuses the reports. The swap
  therefore compares the start state with the view's current state and
  re-sends geometry and presentation that differ, and the view sends its
  current focus through the input queue, in order with later input. A
  replacement no longer registers focused: under the `latest` policy a
  focused registration is the newest focus gain and takes control, and a
  blur sent afterwards would not give control back to the window that
  became active meanwhile.
- Also changed in review: an exit that a replaced view first sees from
  status is the exit transition when the view saw the terminal running
  before its replacement (`replaced_while_running`). The root shell can
  exit after the old viewer's last poll and before the replacement
  subscribes, and core reports `exited` only to a viewer subscribed while
  the shell ran, so the exit-close or the exit notice would otherwise
  never happen. A view first installed after the exit still announces
  nothing.
- Swapping discards the view's `InputQueue`, pending geometry and
  presentation, mouse ownership, and a pending context-menu lookup. It
  restarts `ScrollController` (`restart` clears the latched failure and the
  old viewer's request in flight) and requests a snapshot. A counter of
  the viewers the view has held makes it ignore snapshot and selection
  replies to the old viewer, which would otherwise latch a `Revoked`
  failure on the new one. The window then replaces the tab's activity task
  with one on the new viewer.
- A failed replacement removes the view. A stale error, such as an
  attachment that now belongs to another session, raises nothing; others
  raise "Cannot show tab". The tab then waits, like a failed install, for a
  batch that touches the workspace before an install tries it again.
- The tier 2 test shows the revoked viewer's focus ending and the
  replacement's beginning: the child reads `ESC [ O` then `ESC [ I`. That
  holds when the runtime syncs the revocation before the replacement
  registers, the usual order because the replacement waits for the window's
  drain and a worker. If the owner thread sees both in one batch,
  `Effects::merge` cancels the focus out and in and control passes straight
  to the replacement: the program sees no focus change and the terminal
  does not resize. The tier 2 tests that assert the transitions hold
  replacement by marking the window busy until a fresh observer shows the
  runtime processed the revocation; ten parallel runs without that hold
  failed twice.

Also found in part 1: the refresh smoke detached a view by dropping it while
its tab stayed in the window's workspace, which reconcile now installs
again. The smoke instead moves the tab to another workspace of the session,
so reconcile drops the view as it would after another client's move, and
the terminal stays alive.

### Desktop: exit close has one actor (PR B)

Decided on 2026-10-21 in PR B part 2: `Desktop` does not own an exit-close
outright. The window close pipeline stays the only actor that assesses and
commits a close; `Desktop` owns the owed exit-closes instead of each
window's `ExitQueue`, which is gone. The gating that keeps single-window
behavior correct lives in the window: a busy window, a confirmation that
is showing, merging into a wider `Tabs`, `Window`, or `Application` target,
retrying a refused commit (`close_commit_retries`), the failure notice, and
closing an emptied window (`remove_closed_tabs`). If `Desktop` ran closes
itself, it would have to duplicate all of that or change single-window
behavior. For example, an exit that closed a tab behind a showing
window-close confirmation would make the confirmed commit go stale and
assess again.

`Desktop::exit_closes` (`windows/exit_closes.rs`) keeps one record per
tab, in exit order:

1. **Owed:** the first window whose view reports the tab's exit transition
   records it, owed when that window's config has `close_on_exit` set and
   settled otherwise. A reload therefore affects only future exits, and a
   tab retained by `close_on_exit = false` is never closed later, even by
   a window whose view reports the same exit after a reload. A report for
   a tab the projection no longer holds records nothing, and a record goes
   away when the projection drops its tab, so a busy window that keeps a
   closed tab's view cannot revive its close.
2. **Taken:** `CloseState::next_request` keeps its precedence, queued user
   closes first, then exits. When the window's pipeline is idle, not busy
   and with no confirmation showing, it takes the first owed tab the window
   shows and starts its assessment in the same update. Any window showing
   the tab may take it, however late its view was installed. A window holds
   at most one, and a closing window or teardown takes none. The plan's
   former claimed state, a claim held before the operation starts, does not
   exist: taking starts the operation.
3. **Settled:** the close is done, failed, or retained. Later reports of
   the same exit change nothing.

A taken close ends when the window operation that took it ends:

- A commit that closes the tab settles it, and `TabClosed` lets reconcile
  drop the other windows' views.
- A commit refused with `StaleClose` or `ConfirmationRequired` stays with
  the window while it assesses again.
- A failed assessment or commit settles it with the window's single failure
  notice, and nothing retries it.
- A close that never reached Mux for the tab returns to owed: its target
  merged into a wider one that did not close the tab, such as a window
  close that only detached; the user cancelled a merged confirmation; or
  the retry after a stale commit found the tab gone from the window, as
  when the tab moved to another workspace. The window then offers it,
  through `cx.defer` and window handles as `reconcile` does, to the other
  windows showing the tab, which run `resume_close`. It never reads
  another window's entity from inside its own update.
- Once a commit has started, its worker runs to its result even if the
  window closes. The commit's completion records the result keyed by tab in
  application context, before and regardless of the window's view update,
  so a second window can never assess or commit the same close, and a
  commit that succeeded raises no stale-close notice anywhere.
- When the window is removed, a close it took and had not started
  committing returns to owed and is offered to the others.

A window that drops its view of the tab while its own assessment runs
still commits the close; the runtime decides whether the tab is still
there. Only the stale-commit path above returns such a close to owed.

Exit notices stay per window, based on the viewer's exit transition, so a
view installed after the exit announces nothing. Failure notices stay per
window too.

An exit-close never asks for confirmation. An exited terminal's job
assessment is always idle (`JobLifecycle::assess` in `huterm-core`), and
only a non-idle assessment asks, so no production path raises a dialog for
an exit-close. Decided before PR B: PR B adds no confirmation rules for
exit-closes and no test-only way to produce a confirming assessment.
Instead, the tier 2 tests pin that an exit-close shown in two windows
closes without prompting. If an exited terminal can ever assess as
non-idle, the two rules agreed on 2026-10-03 return with it: cancelling
clears the owed close for every window, and a window closed while its
confirmation shows returns the close to owed. Today a cancelled merged
confirmation returns the exit-close to owed, which a later refresh of that
window takes again.

Tier 2 tests hold or fail the assessment and the commit through a
test-only hook in `DesktopRuntime` (`windows/worker_hooks.rs`, compiled
only for tests with the `test-support` feature). It counts both stages,
holds either until released, and fails the next worker that passes. A
failed commit runs the real commit first, as a teardown failure after the
runtime accepted the close would.

### Smoke fixture (PR B)

The palette smoke fixture gains `attach-window\t<index>`. It opens a window
without a shell (`launch_shell = false`), attaches it to the session of the
window at `index` through `Mux::attach_session`, records the attachment,
session, and workspace in the window model, and runs that window's full
reconcile, which installs the views. Production code gains no command for
this.

A second fixture command revokes a window's viewers, the only way to reach
in-place re-subscription before [#21][issue-21] and [#26][issue-26]. Built
in part 3 as `revoke-window\t<index>`: on a worker, under one Mux lock, it
creates a session without workspaces, retargets the window's attachment to
it and straight back, and closes it. Retargeting revokes the attachment's
viewers and emits no hierarchy event, and the temporary session closes with
nothing attached, so the projection's structure is unchanged; the tier 2
test checks its digest. Tier 2 tests call the same function,
`palette_smoke::revoke_viewers`.

A new `check-viewers.ts` runs inside the macOS and Linux palette smokes, as
`check-overlays.ts` does (see Verification), as its own `viewers` CI step.
It addresses windows by index through fixture commands instead of native
input, so it never needs a particular window to hold native keyboard focus:
`type-in` sends a line through a window's input queue, `invoke-in` runs a
catalog command in a window, `resize-window` resizes one, and `activate`
activates one. `viewport` reads the shared viewport and the canonical grid
through a fresh observing viewer, so the read rearms neither window and
needs no frames. The PTY size comes from the shell, which appends
`stty size` to a file on request; a request typed in the controlling window
is ordered behind every geometry report queued before it, which makes it
the boundary for "the size did not change".

Decided on 2026-10-21 in PR B part 1: the snapshot counter is derived in the
desktop instead of core, so PR B keeps one core change. Under the smoke, a
global counts each terminal's snapshot replies across views: a reply that
carries the same `Arc` as that terminal's previous reply reused the cache,
and any other built a snapshot. Replies to requests outside the desktop's
views are not seen, which a report, not a gate, can accept. Smoke state
reports it as `w<i>.snapshots=<builds>,<reuses>`, next to
`w<i>.clipboard_writes` for the OSC 52 check.

The palette smoke fixture is the production application plus file-driven
commands, so PR B also adds a helper script that launches it and attaches a
second window, for manual checks of Cmd-Tab, Mission Control, and occluded
windows: `mise run smoke:manual-viewers`.

### Contract items for #44

The in-process implementation needs none of these, but the types must leave
room so [#44][issue-44] adds messages instead of changing semantics:

- `ViewerId` is scoped like the other IDs and needs server incarnation,
  like `StreamId`.
- Status is state with a revision. A connection sends the latest status
  after a lag or reconnect; there is nothing to replay.
- Snapshots carry their publication and geometry revisions as the baseline
  for row deltas. A skipped revision is never an error, because every
  snapshot is complete.
- The server checks capabilities and revocation per message.
- Scroll numbers for return-to-live are per viewer and assigned by the
  client, so the rule needs no terminal-wide counter across the wire.
- Per-connection input byte budgets replace the shared 1 MiB budget, so one
  connection's paste cannot refuse another's keystrokes.
- A disconnected viewer is dropped, not revoked: its admitted input still
  runs, then it is finalized and its mouse gesture released.
- A recipient that never drains holds the terminal's host-effect budget of
  8 effects and denies later writes for every recipient. Remote recipients
  need a delivery deadline or per-recipient budget.

## Delivery

### PR 1: core viewers ([#205][issue-205])

1. Protocol: add `ViewerId`, `ViewerCapabilities`, `TerminalStatus`, and the
   lifecycle and failure records; add the publication and geometry
   revisions to `TerminalSnapshot` and an `InputStamp` (the viewer's scroll
   count and an optional geometry revision) to input; remove `TerminalEvent`
   and `TerminalInput::Focus`.
2. Core runtime: viewer slots and registry, arbitration messages,
   per-viewer publication and rearm, status publication, the snapshot cache,
   viewport publication and return to live on input, focus aggregation, the
   controller with `latest` sizing and presentation, `MouseArbiter`,
   capability checks, finalization, and `NotPermitted` and `Revoked`.
   Remove `EventPublisher`, `PresentationAuthority`, and
   `PresentationController`.
3. Mux: `subscribe_terminal`, revocation at every existing authority
   invalidation point, and the crate-private client for close tickets.
4. Desktop: migrate `TerminalView`, its activity task, spawn registration,
   pending arbitration state, scroll intent on typing, GPUI tests,
   benchmarks, and examples to one viewer per view.
5. Documentation: the agent guides, CONTEXT.md, and the plan references
   below.
6. `pty::spawn` serializes PTY creation with a process-wide lock.
   Concurrent `openpty` calls in one process failed on macOS 27 under the
   new parallel PTY tests. Mux already serializes production spawns, so
   the lock costs nothing there.

### Desktop multi-viewer ([#206][issue-206]): two PRs

Each PR is safe to merge alone. PR A changes nothing a single viewer can
observe beyond the one-snapshot interval described above. PR B lets a second
window show a session, through the fixture only, and closes both problems
that a second window brings: a tab closed by two windows, and a revoked view
left without a viewer.

PR B starts from PR A's branch and targets `main` once PR A merges. Plain
branches suffice; the stack needs no `gh stack`.

#### PR A: requested and displayed grids

All changes are in `crates/huterm-gpui`, plus the task and lockfile changes
the test feature needs. No core or protocol change is expected.

1. Harness spike. Add the `test-support` feature to `huterm-gpui`, pass it
   from the Cargo commands in the `lint`, `typecheck`, and `test:rust`
   tasks, add the script test that checks they do, and write the tier 1
   helper with one test: a `TerminalView` in a test window over a
   standalone runtime shows its child's output, and a simulated click
   reaches the child as one press and one release. Run it on macOS. Run it
   in the Linux container if `df` shows enough free disk; otherwise the
   Linux answer comes from CI once the PR is pushed, and the recorded
   result says it covers macOS only until then. Run `mise run license`.
   Record the result in the harness section above, and stop for the
   maintainer if a real `TerminalView` cannot be driven.
2. Add `GridGeometry` in its own module under `desktop/`, with its unit
   tests. `TerminalLayout` keeps computing the requested grid and bounds.
3. Rename `last_grid_size` to `requested_grid` and route each consumer in
   the table above through the displayed grid or the visible area:
   `application_mouse` and `link_cell`, the selection edge in `mouse_move`,
   `scroll_command` and the scrollbar track press, `scrollbar_geometry`, the
   scroll benchmark's page size, `render_size_panel`,
   `context_menu_anchor`, and `bounds_for_range`. `resize_to_layout`,
   `pending_resize`, and `physical_cell_size` stay on the requested grid.
4. Stamp mouse reports in `admit_input` with the displayed snapshot's
   geometry revision.
5. Forget the motion sample when `apply_snapshot` sees a new geometry
   revision, and forget the hover point where `resize_to_layout`
   invalidates link state.
6. Report both grids from the five smoke state writers, move the refresh
   smoke to `requested_grid`, and make `check-presentation-queries.ts` and
   `check-fullscreen.ts`, which already compare `grid=` with the PTY size,
   also require a visible tab's displayed grid to match once settled. The
   presentation query smoke reloads its configuration while tab 0 is
   hidden; that step keeps comparing only the requested grid.
7. Add the tier 1 tests under Verification.
8. Update the guides under Documentation.

#### PR B: installing, closing, and replacing views

A core commit, then three parts, landed in this order as separate commits,
so each part's tests pass before the next begins:

0. Arbiter. `MouseArbiter` accepts motion that names a held button only
   from the gesture's owner, with its unit test. This is PR B's only core
   change, so `mise run test:ghostty-releasesafe` runs for it.

1. Installing views. Add install and session-change removal to reconcile
   and `WorkspaceView`, and the full reconcile for a window that gains an
   attachment. Add the `attach-window` fixture command, `check-viewers.ts`
   steps 1 to 5 and 7 wired into the macOS and Linux palette smokes, a
   debug counter for snapshot builds and cache reuses reported in smoke
   state, derived in the desktop (see "Smoke fixture"), and the helper
   script for native two-window checks. Extend the harness to tier 2.
2. Exit close. `Desktop` owns the owed exit-closes and the window close
   pipeline takes them; `Desktop` does not own the close itself. Pin the
   single-window exit behavior in tier 2 first, then add the exit-close
   test hook, the tier 2 exit-close tests including the one that an
   exit-close never prompts, and `check-viewers.ts` step 6.
3. Replacement. Re-subscribe a revoked view in place from reconcile. Give
   `ScrollController` a way to clear its latched failure (`restart`), so a
   replaced viewer resumes snapshots. Add the `revoke-window` fixture
   command, cover replacement through it in tier 2, and add a replacement
   check to `check-viewers.ts`.

### Follow-up issue: sizing policies ([#207][issue-207])

Add `smallest`, `largest`, and `manual` to the selection function, a config
setting, the schema, and a command for a manual size. Define row and column
aggregation, cell size under mixed scales, and how manual size interacts
with viewers' requests. It does not block [#26][issue-26], [#28][issue-28],
or [#44][issue-44].

## Verification

### PR 1

Pure unit tests, without a PTY:

- Controller selection table: activity, ties, a hidden viewer keeping
  control over an idle shown one, a single hidden viewer still controlling,
  viewers without `size`, dropped viewers leaving candidacy at once, revoked
  and finalized viewers, no candidate keeping size and presentation, and
  input that moves control resizing before it writes. Mouse reports never
  move control.
- Focus aggregation: two viewers focusing and blurring in each order,
  including the first viewer's blur before the second's focus, emit the
  expected focus in and out; finalizing a focused viewer emits focus out;
  viewers without `input` never count; a viewer with `input` and no `size`
  is accepted and counts.
- `MouseArbiter`: interleaved presses from two viewers; motion and wheel
  during another viewer's gesture; an unmatched release; a press, hover
  motion, and wheel report with a stale geometry revision, including a
  resize there and back; a press followed by a canonical resize and then
  its release, which still ends the gesture; synthetic releases when the
  owner is finalized.
- Status: a repeated identical title, metadata, or failure leaves the
  revision unchanged; bells and failure sequences from a late subscriber's
  baseline; a failure gap reports the dropped count; a `spawned` baseline
  reports an exit that happened before the first poll.
- Publication: a notified viewer is not woken again until its snapshot is
  built; building one viewer's snapshot does not clear another's flag; a new
  slot starts notified.
- Finalization: drop and revocation together finalize once; a refused send
  does not leave the queued count above zero.

PTY integration tests in `huterm-core`, two viewers on one terminal unless
stated:

- Both viewers are woken by output. A viewer that never requests a snapshot,
  standing in for a suspended or slow client, does not stop parsing or the
  other viewer from seeing a later marker; when it finally requests, its
  snapshot is complete and current.
- A viewer subscribed after output and after exit reads the current title
  and exited status, and its first snapshot is complete.
- A command that exits immediately still gives the `spawned` viewer an exit
  transition.
- After root exit, a geometry report still resizes the emulator, and the
  next snapshot shows the new size.
- A scroll from viewer A wakes viewer B; B's snapshot shows A's viewport at
  the same content generation, and a selection extracted at that generation
  still succeeds.
- Text input from B while A holds the viewport in history returns the
  shared viewport to live, without B requesting a snapshot.
- A key queued behind a large output backlog, followed by a scroll submitted
  after it, leaves the viewport where the scroll put it.
- Markers sent concurrently from both viewers each arrive intact and both
  arrive.
- A viewer without `input` gets `NotPermitted` for input, without `viewport`
  for scroll, and without `size` for geometry; the input byte budget is
  unchanged afterwards. A viewer with `size` but not `input` takes control
  by focus.
- Geometry from the viewer that does not control leaves `stty size`
  unchanged; focus on that viewer moves control and changes `stty size`.
  With one viewer, hiding it and then reporting new geometry still changes
  `stty size`.
- Dropping the controlling viewer with queued input changes `stty size` at
  most once, to the remaining viewer's geometry.
- Revoking a viewer through `detach_session` while it holds a queued
  snapshot request answers that request with `Revoked`; the other viewer's
  next snapshot succeeds and the child is still alive.
- Revoking an idle focused viewer that holds a mouse button writes focus
  out and a release without any other activity.
- Dropping a viewer with a queued press writes the press and then its
  release, in that order; dropping a viewer still writes its queued text.
- A viewer that registers, presses, and drops inside one owner turn has its
  press written and released between its focus in and focus out.
- A press queued after the gesture owner drops, inside the same owner turn,
  is written after the owner's synthetic release instead of being refused.
- Threads that subscribe and drop viewers continuously do not stop typed
  lines from being echoed; each line is typed only after further churn.
- While writes are backed up, subscribe-and-drop churn reaches the viewer
  limit and leaves the backlog unchanged; once the writes drain, the dropped
  viewers are finalized without a further request.
- A single viewer's focus and blur each write exactly one `ESC [ I` and one
  `ESC [ O`.
- A clipboard write reaches exactly one recipient across focus handoff,
  hiding, dropping, and revocation, and wakes that recipient's viewer.
- Alternate-screen entry while a viewer is scrolled keeps both viewers'
  viewports equal. History eviction is not automated: the engine retains
  16 MiB of history, so evicting it needs more output than a unit test
  should produce, and the engine anchors one shared viewport either way.
- A title-only change is visible through status without a new snapshot.
- The 33rd subscription fails, and succeeds again after one viewer is
  finalized.
- Closing the terminal with live viewers still terminates and reaps the
  child, and both wakes close after the final status.
- A runtime panic closes every wake and releases the PTY, so a child that
  keeps writing is hung up and reaped. A panic does not terminate an idle
  child; [#211][issue-211] tracks that.
- A new title in PTY output arms a foreground probe when the output alone
  would not.

Perturbation checks, run once and restored, each required to fail at its
intended assertion:

- Remove the publication from viewport moves: the cross-viewer scroll test
  must fail because the cache returns the old snapshot.
- Remove the publication from controller-driven resizes: the handoff test
  must fail when it checks B's snapshot size after focus moves control.
- Clear every viewer's flag on snapshot construction instead of only the
  requester's: the test that one viewer's snapshot leaves the other notified
  must fail.
- Ignore the scroll submission number: the backlog key-then-scroll test
  must fail at its viewport assertion.

Desktop coverage. `huterm-gpui` has no harness that drives a
`TerminalView` against a live runtime, so PR 1 covers the desktop with
unit tests of its queues and controllers, the core PTY tests, and the
smokes. A view-level harness lands with PR A and grows in PR B, which needs
it for reconcile.

- `InputQueue` unit tests: focus entries keep their order around input,
  coalesce only when adjacent, ignore capacity and exit closure, and a
  refused focus change holds back later input.
- `ScrollController` unit tests: typing drops pending intent but keeps
  later scrolls, so an older completion cannot replay into history.
- The core PTY tests cover the runtime half of each desktop path: focus
  and mouse byte order, hidden single-viewer resize, resizing after root
  exit, return to live in both arrival orders, and status publication.
- `smoke:macos-refresh` covers title delivery while frames are paused and
  activity-task cancellation on detach. The Linux desktop integration
  smoke covers the visual bell, exited tabs kept by
  `close_on_exit = false`, and process and directory labels. The
  presentation query smoke covers hidden-tab padding, font, and theme
  reloads reaching the PTY.

Moved to PR B's harness, which hosts the production window root that typed
text needs on macOS: the refresh applying each status change exactly once,
a view installed after exit reporting no transition, an exited tab
reflowing through its view, and a focus gain, key, and blur draining
through a real viewer as `ESC [ I`, the key's bytes, and `ESC [ O`.

### PR A

`GridGeometry` unit tests, without a window, each against a displayed grid
larger than, smaller than, and equal to the requested one:

- The inside test accepts every position in the visible area and rejects
  the padding, the background beside a smaller grid, and positions past the
  clip of a larger one. The equal case keeps today's
  `application_coordinates_exclude_titlebar_padding_and_grid_endpoints`
  expectations.
- Coordinates outside clamp to the displayed grid's nearest edge: column and
  row zero to the left and above, and the last column and row to the right
  and below, which lie beyond the visible area when the grid is larger.
- A viewport narrower or shorter than one cell accepts positions on the
  part of the cell it paints, as today. Bounds that padding reduces to
  nothing accept none, with and without `padding_balance`. Both still page
  by one row.
- The page size is the smaller of the requested and displayed rows, and the
  displayed rows give the scrollbar geometry.
- Selection points clamp to the displayed grid, and the edge direction uses
  the visible area's height.
- With `padding_balance`, the displayed grid's origin is the requested
  layout's origin.
- A cursor cell beyond the clip has no visible position, so the keyboard
  context menu and the IME candidate rectangle anchor at the grid's
  top-left corner.

Two more unit tests:

- The stamp: a mouse report takes the displayed snapshot's geometry
  revision, and keys, text, and paste take none, so a stale snapshot never
  discards typing.
- `Links`: after its state is invalidated and its hover point forgotten, an
  update with the same point requests a lookup. Invalidation alone does
  not, which is today's behavior and the reason for the change.

Tier 1 harness tests, each with a real runtime whose child enables mouse
tracking and echoes the reports it receives. The boundary cases stay in the
unit tests; these prove the view is wired to them. A report that must not
arrive is shown absent by a later report that arrives alone, so every
negative assertion ends at an ordered boundary:

- The spike test: a click inside the grid arrives as one press and one
  release at the cell under the pointer.
- The one-snapshot interval, which is the test of the stamp. The test
  starts settled: the displayed grid equals the requested grid and the
  view's queue is empty. Inside one window update, by calling the view's
  handlers directly, it changes the layout so the requested grid changes,
  checks that the geometry report was admitted and `pending_resize` is
  empty, and presses and releases on a cell inside both grids. No snapshot
  can be applied inside that update, so the press carries the old revision
  and follows the resize on the runtime's ordered queue. The test then
  waits for a snapshot with the new revision, presses and releases on a
  different cell, and waits for that report. The child's output holds the
  second cell's report and nothing for the first.
- A hover report discarded as stale does not suppress the next one. A bare
  viewer controls, and the child tracks all motion. Inside one window
  update the bare viewer reports the same grid with a different cell size,
  and the view then receives a motion event on cell X. The resize runs
  first on the runtime's queue, so the motion carries the old revision and
  is discarded. The test waits until the view shows the new revision,
  sends a second motion on cell X with no press, release, wheel, or focus
  change in between, since each of those also clears the sample, and then
  presses. The child's output holds one motion at X followed by the press.
  The change of cell size alone shows that the view compares revisions,
  not sizes.
- A larger canonical grid, and a gesture that spans the change. The view
  presses on cell A while the grids agree. A bare viewer then takes control
  with a larger grid, and the test waits for the view to show it: the
  view's requested grid is unchanged and its displayed grid is the
  canonical one. A release far outside the window arrives clamped to the
  canonical grid's last column, which shows the gesture survived the new
  revision. A press from the bare viewer is then accepted, a press from the
  view on a visible cell B arrives at B, and a page-up command moves the
  viewport by the view's requested rows.
- A smaller canonical grid: a press and a hover motion on the background
  beside the grid write no report, shown by a later press inside the grid
  arriving alone, and a wheel event there scrolls the viewport locally.
- A hovered link under a stationary pointer is found again after the view's
  own layout changes while a bare viewer keeps control, so the canonical
  grid stays as it was and no snapshot follows the change.

Not tested in PR A:

- A report dequeued before a geometry report that waits in
  `pending_resize`. Its order comes from `retry_client_messages`, which
  PR A does not change, and forcing a busy runtime in the harness costs
  more than the case is worth.
- The size panel's text. It reads the displayed grid, and the interval test
  already waits for the displayed grid to equal the requested one.

Perturbation checks, run once and restored, each required to fail at its
intended assertion:

- Stamp mouse reports with no revision: the one-snapshot interval test must
  fail because the first press arrives.
- Map against the requested grid: the smaller canonical grid test must fail
  because the background press arrives.
- Skip the motion reset: the stale hover test must fail because no motion
  precedes the press.

The hovered link test needs no perturbation: it is written before the hover
point is forgotten and must fail first.

The harness tests exist only with the `test-support` feature, so the PR
names each one from the output of `mise run test`, to show the runner
collected them.

Smoke and benchmark evidence:

- The host macOS smokes whose state changes: `smoke:macos-palette`,
  `smoke:macos-integration`, `smoke:presentation-queries`,
  `smoke:macos-fullscreen`, `smoke:macos-quake`, and `smoke:macos-refresh`.
  The integration and fullscreen smokes enable mouse tracking and assert
  the reports a program receives, so they cover the stamped path in the
  production window. `smoke:macos-input` covers keyboard input and local
  selection.
- `mise run linux:smoke` for the same writers and for mouse reports under
  Xvfb, where frames arrive only on request, if `df` shows enough free
  disk. Otherwise CI's Linux smoke job is the evidence, and the PR says so.
- `mise run ci:benchmarks` before and after, to show the input, scroll, and
  link budgets hold.

Commands: `mise run check`, `mise run test`, and `mise run license` while
working, and `mise run verify` before handoff. Run
`mise run test:ghostty-releasesafe` only if core changes.

### PR B

Installing views:

- The view-level tests moved from PR 1, listed above, in the tier 2 harness,
  plus one that two attached windows install and show a session's tab,
  typing in one reaches the other, and a tab opened later installs in both
  behind the tab each shows.
- Reconcile tests in `windows/projection.rs`: a projected tab without a view
  yields one install; an install in flight, a busy window, or a terminating
  runtime yields none; reconciling twice yields no duplicate; a tab removed,
  or whose viewer was revoked, before the completion is not pushed; a tab
  whose workspace moved to another session is removed. A failed install
  waits for a batch that touches its workspace. The revoked case has no
  test: nothing deterministic can revoke a viewer between the worker and
  its completion, and the check reuses the constructor's request.
- `check-viewers.ts` steps 1 to 5 and 7, below, and the OSC 52 assertion.

Exit close:

- Tier 2 tests that pin today's single-window behavior, added before the
  refactor and passing on both sides of it: an exited tab closes under
  `close_on_exit`, the last tab's exit closes its window, and an exit while
  the window is busy closes once the busy close ends.
- Tier 2 exit-close tests, with both windows observing the exit before the
  close is decided unless noted:
  - exactly one close is assessed and committed, and the other window takes
    nothing while the first holds it;
  - a view installed after the exit takes a close that its busy window
    cannot, and takes it over when the claiming window closes during its
    assessment, then closes the tab;
  - with the assessment held and then the commit held, dropping the
    claiming view starts no second operation;
  - closing the claiming window after its commit starts and before the
    commit returns leads to one close and no notice in the other window;
  - an assessment failure and a commit failure each settle the close with
    one notice in the claiming window and none in the other;
  - a tab that exited under `close_on_exit = false` stays open after both
    windows enable it;
  - a view installed after the exit announces nothing (part 1's test);
  - no window shows a confirmation for any of these closes.
- Unit tests for `ExitCloses`: the first report decides, takes follow exit
  order with one close per window, an uncommitted close returns to owed
  while a committing one waits for its result, a refused commit stays with
  an open window, and records end with their tabs.
- `check-viewers.ts` step 6, below.

Replacement:

- Tier 2 replacement tests through `revoke_viewers`, the fixture command's
  mechanism:
  - a revoked view is replaced in place, keeping its entity, the active
    tab, and focus; the child reads `ESC [ O` then `ESC [ I`; the hierarchy
    digest is unchanged; and the replacement controls the size again after
    a window resize;
  - a replacement that does not resize keeps the selection;
  - after replacement, new output, a title change, a clipboard write, and
    an exit each reach the view with no display frame delivered, under
    `refresh = "unlimited"` so snapshots wait only for the activity drain;
  - a view whose re-subscription fails is removed without a notice. The
    test retargets the window's attachment to another session and leaves
    it there, a real path that emits no hierarchy event;
  - replacing the controlling viewer while another window's viewer has a
    different size advances the geometry revision by exactly two and clears
    the selection;
  - with the replacement held between its start and its subscription by a
    `Subscribe` gate in the test-only worker hooks
    (`windows/worker_hooks.rs`), a window that blurs while the other
    window takes focus leaves focus and the size with the active window,
    and a window resized meanwhile gets the current size;
  - a shell that exits while its only live view is being replaced still
    closes the tab under `close_on_exit` and raises the exit notice
    without it.
- A `ScrollController` unit test: restarting after `Revoked` lets the next
  invalidation start a snapshot request, also while a request was in
  flight.
- A reconcile unit test: a revoked view whose tab stays is replaced once,
  not while busy and not again while in flight; one whose tab moved is
  removed; a failed replacement waits for a batch that touches the
  workspace.
- `check-viewers.ts` revokes window 1's viewers and asserts that output
  from typing in window 0 still reaches window 1.

### The viewers smoke

- `check-viewers.ts` in `smoke:macos-palette` and `smoke:linux-palette`.
  PR B adds all seven steps.
  Assert runtime-backed state where Xvfb delivers no frames: the PTY size
  from the shell and the shared viewport from a fixture snapshot request,
  not from window 0's rendering.
  1. Attach window 1 to window 0's session and assert both windows show the
     tab.
  2. Type a marker in window 0 and assert it reaches window 1's view.
  3. Scroll in window 1 and assert the shared viewport moved and window 0's
     displayed offset follows on macOS.
  4. Shrink window 1 while window 0 is active and assert the PTY size is
     unchanged; activate window 1 through the `activate` command, not a
     grid click, and assert the PTY size follows window 1.
  5. Open a second tab in window 0 and assert window 1 installs it.
  6. With `close_on_exit`, exit the first tab's shell and assert one close:
     both windows lose that tab, keep the second tab, and show no
     stale-close notice.
  7. Close window 1 and assert window 0's shell still answers.
- Between steps 6 and 7, revoke window 1's viewers with `revoke-window`
  and assert that output typed in window 0 still reaches window 1.
- Write OSC 52 from the shared terminal and assert from smoke state that
  exactly one window delivered it. The core test owns the exact-once
  guarantee across handoff; the smoke proves the desktop wiring.

### Efficiency gates

Run before and after PR 1. PR A reruns only `ci:benchmarks`, because it
changes input mapping and page scrolling and nothing an idle terminal runs.

- `mise run ci:benchmarks`: output latency within the 5 ms applied budget,
  at most 1.5 snapshots per key, scroll and link budgets unchanged. The
  publication lock and the runtime's return to live must stay within these.
- `mise run bench:idle` on macOS with an unlocked, unoccluded screen: idle
  wakeups and CPU unchanged.
- A core test floods a terminal with a re-sent unchanged title and asserts
  that a hidden viewer receives no wake after its first.
- For PR B, report snapshot builds and cache reuses for scrolling and for
  an output burst with one and two visible viewers, from the debug counter
  in smoke state. This is a report, not a gate. `check-viewers.ts` prints
  it as its `VIEWERS_SMOKE` line.

Commands for PR 1 and PR B: `mise run check`, `mise run test`,
`mise run test:ghostty-releasesafe` when core changes, the macOS host smokes
(never the Tart VM smokes, which need about 80 GB), and
`mise run linux:smoke`. Check free disk with `df` before a Docker run, and
clean up its volumes and images afterwards. Run `mise run verify` before
handoff. PR A's commands are listed with its verification.

## Documentation

- `docs/agents/core-runtime.md` and `docs/agents/desktop-client.md`: replace
  the single-consumer event and activity rules with the viewer rules:
  per-viewer rearm at snapshot construction, status as state,
  capability checks at dequeue, viewer errors never fatal, focus changes
  exempt from input limits and never overtaking earlier input while
  geometry stays latest-wins,
  accepted gestures always ending, and the controller deciding size and
  presentation.
- `CONTEXT.md`: add Viewer and Controlling viewer.
- `docs/plans/workspaces-windows-tabs.md`: "Keep viewport dimensions per
  attachment" becomes per viewer, pointing here.
- `docs/plans/event-driven-refresh.md`: note that cloned clients no longer
  exist and each viewer has its own wake.
- `docs/plans/hierarchy-projection.md`: mark the install deferral done and
  move attachment events to [#26][issue-26] and [#21][issue-21].

PR A adds:

- `docs/agents/desktop-input.md`: mouse reports, link points, and selection
  map against the displayed grid and clamp to it; mouse reports carry the
  displayed snapshot's geometry revision; the paint clip stays at the
  requested grid's bounds, which now also clips a larger canonical grid.
- `docs/agents/desktop-client.md`: `TerminalLayout` owns the requested grid
  and `GridGeometry` the mapping onto the displayed grid; `requested_grid`
  is what the view reports, never what it draws.
- `docs/agents/desktop-smokes.md`: `grid=` is the requested grid and
  `displayed_grid=` the snapshot's.
- `docs/agents/development.md`: the GPUI test harness, its `test-support`
  feature, which tasks pass it, and its tier 1 limits.
- `docs/agents/dependencies.md`: GPUI's `test-support` feature is enabled
  only through `huterm-gpui`'s feature, never as a dev-dependency, and why.

## Risks

- **A snapshot-affecting change that skips the publication** would let the
  cache serve stale content. Keep one publication function and the
  perturbation tests.
- **Resize churn under `latest`.** Moving focus between differently sized
  views on one terminal, including a quake window and a regular one,
  resizes the PTY, which reflows scrollback and clears selections in every
  view. This matches tmux's `latest`; the follow-up policies give users an
  alternative.
- **Focus events need drawn frames.** GPUI raises them on a drawn frame, so
  a window that draws nothing reports its blur late. Native testing should
  cover Mission Control, Cmd-Tab, and occluded windows beyond the smoke's
  `activate` step.
- **Installing and replacing views from reconcile** crosses a worker, a
  sequence wait, and a deferred window update. Each step re-checks the
  projection and revocation, and no step reads another window's entity or
  handle.
- **Removing `TerminalEvent`** changes a public protocol type. Nothing
  outside this repository consumes it yet; [#44][issue-44] has not shipped.
- **One shared input byte budget** lets one viewer's large paste refuse
  another viewer's input until it drains. Acceptable in process;
  [#44][issue-44] owns per-connection budgets.
- **The stale-geometry check becomes live in PR A.** Until then no report
  carries a revision. A snapshot that stayed stale after a resize would make
  the runtime discard every press from that view. Every applied resize
  publishes, including one that changes only the cell size, and a publication
  invalidates the view, so a visible view that receives frames converges on
  the new revision. A hidden or occluded view can stay stale, and it
  receives no pointer input. A smoke that clicks within a frame of a layout
  change could now lose that click; the smokes run to find out. The
  integration and fullscreen smokes are the check that reports still arrive
  in the production window, including under Xvfb, where frames arrive only
  on request.
- **The tier 1 harness is not the production window.** Its drain copies the
  tab activity task's shape, its font metrics are synthetic, and its root is
  not `WorkspaceView`. It proves view behavior against a real runtime; the
  smokes still own the production wiring.
- **A second GPUI build.** Tests build GPUI with `test-support` and smoke
  binaries build it without, so a checkout that runs both holds two GPUI
  builds. A dev-dependency would not save one: `cargo build -p huterm`
  builds GPUI without dev-dependency features, so it would differ from the
  test and example builds instead.

## Unresolved questions

- None blocking. Mouse arbitration may change after experiments, within
  `MouseArbiter`, and the stale-display wheel limitation above is accepted
  unless the maintainer objects.

The three questions raised during PR A were decided by the maintainer
before PR B: the arbiter discards held-button motion from a viewer that owns
no gesture, an exit-close never prompts, and replacement keeps the selection
only while the content generation is unchanged. Each decision is recorded
in its section above.

[issue-9]: https://github.com/jimeh/huterm/issues/9
[issue-20]: https://github.com/jimeh/huterm/issues/20
[issue-21]: https://github.com/jimeh/huterm/issues/21
[issue-22]: https://github.com/jimeh/huterm/issues/22
[issue-26]: https://github.com/jimeh/huterm/issues/26
[issue-28]: https://github.com/jimeh/huterm/issues/28
[issue-44]: https://github.com/jimeh/huterm/issues/44
[issue-45]: https://github.com/jimeh/huterm/issues/45
[issue-48]: https://github.com/jimeh/huterm/issues/48
[issue-97]: https://github.com/jimeh/huterm/issues/97
[issue-98]: https://github.com/jimeh/huterm/issues/98
[issue-134]: https://github.com/jimeh/huterm/issues/134
[issue-205]: https://github.com/jimeh/huterm/issues/205
[issue-206]: https://github.com/jimeh/huterm/issues/206
[issue-207]: https://github.com/jimeh/huterm/issues/207
[issue-211]: https://github.com/jimeh/huterm/issues/211
