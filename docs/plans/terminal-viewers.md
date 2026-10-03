# Terminal viewers

Status: approved for [issue #22][issue-22] after three adversarial review
rounds. PR 1 delivers [#205][issue-205]; PR 2 delivers [#206][issue-206].

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

- Deliver in two PRs, core viewers and then desktop multi-viewer, and move
  the smallest, largest, and manual sizing policies to a separate issue.
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
  AGENTS.md requires. After root exit the runtime still resizes the
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
baseline after it is registered. The owner thread reconciles its viewer
table before each viewer request as well as on each turn, so reports a new
viewer queues before the owner's next turn still find its entry. Viewer IDs
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
owner thread checks again at dequeue: it discards input, edits, and
arbitration reports from a revoked viewer or one whose capabilities do not
permit them, releasing their byte reservation, and answers a refused
scroll with `NotPermitted`. In process, capabilities are immutable, so the
handle's check normally decides and the dequeue check matters for
revocation. Over [#44][issue-44] the server repeats the capability check
per message, so enforcement does not depend on client code.

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
- Motion and wheel reports with no gesture held are accepted from any viewer
  with `input`.
- Every input carries an `InputStamp` recorded when the user acted. Its
  optional geometry revision names the snapshot that mouse coordinates came
  from. If the canonical geometry changed since, presses, motion with no
  gesture held, and wheel reports are discarded, so a resize between
  computing a report and dequeuing it cannot move it to another cell. A
  revision, unlike a grid size, also catches a resize there and back. In
  PR 1 the desktop computes mouse coordinates from its own layout and sends
  no geometry revision; PR 2 stamps reports once it maps them against the
  displayed grid.
- Motion and releases inside an owned gesture are never discarded for
  geometry; the encoder clamps their coordinates to the current grid, as it
  does today. Accepted gestures therefore always end.
- When the owner viewer is finalized with buttons held, the runtime writes
  releases for those buttons at the owner's last reported position. Only
  the owner's reports move that position.
- When the application turns mouse tracking off during a gesture, the buttons
  held then stop blocking other viewers. Their releases are still the owner's,
  including after the owner presses again, and another viewer's next admitted
  press abandons them; a refused press, such as one with a stale geometry
  revision, leaves them in place. A view that saw tracking stop never sends
  their releases, so other viewers' presses would otherwise stay discarded once
  the application tracks the mouse again. A view that did not see it, because
  tracking came back on before its next snapshot, still sends them, and the
  runtime still writes them. A report dequeued while tracking is off, such as a
  press computed against an older display, writes nothing and takes no gesture.

A view whose press was discarded keeps its local ownership state until its
own release and gets no feedback; that is acceptable for the default. The
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
thread reconciles before it next waits. The notification that asked for
the reconcile was consumed during the backlog, so nothing else is
guaranteed to wake it.

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
- `Revoked` shows no notice. The view stops sending, and PR 2's reconcile
  replaces or removes its viewer.

PR 1 intends no user-visible behavior change.

### Desktop: the displayed grid follows the canonical size (PR 2)

`TerminalLayout` keeps computing the grid this view **requests** from its
bounds. A view now displays the snapshot's `size`, which can differ when
another viewer controls:

- The grid paints from the layout origin at the canonical size and clips to
  the available area. A larger canonical grid is cut off at the edge, and a
  smaller one leaves background.
- Page scrolling, selection-edge rows, and link points use the displayed
  grid.
- A new press, hover motion, or wheel report outside the displayed grid
  sends nothing. Motion and the release of a gesture the view already owns
  are sent with clamped coordinates wherever the pointer is, as today.
- Mouse reports carry the displayed snapshot's geometry revision for the
  arbiter's check.
- The size panel shows the canonical size.
- `last_grid_size` splits into the requested grid and the displayed grid.
  Smoke state reports both, so existing `grid=` assertions keep meaning the
  PTY size.

Each view still renders with its own font metrics and scale. Only the cell
count is shared.

### Desktop: installing, replacing, and removing views (PR 2)

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
   tab, without taking focus from an overlay.

A subscription that fails because its target is stale is dropped silently
and not retried until a later batch touches that workspace, so a failure
cannot loop. Once the runtime is terminating, install failures show nothing;
before that, unexpected errors show a notice. Closing the window drops the
in-flight task and the viewers it returns.

When a view's viewer reports revocation and the projection still holds its
tab in the window's workspace, the window re-subscribes **in place**: the
same worker and sequence wait produce a new viewer, which replaces the old
one inside the existing `TerminalView`. Selection, scroll state, and the
active tab are kept. The new viewer is created with the view's current
focus, geometry, and presentation as its initial state, because
GPUI raises no new focus event for a view that stays focused; otherwise the
old viewer's finalization would leave the application believing the
terminal lost focus. The tab's activity task waits on its viewer's wake, so
installing the replacement cancels that task and starts a new one on the
new viewer; otherwise output, status, and host effects for the replacement
would have no consumer. Input still waiting in the view's `InputQueue` is
discarded, since the old viewer's authority ended. If re-subscription fails,
the view is removed.

Removal gains one case: a tab whose workspace now belongs to a session other
than the window's attachment session is removed, as a tab moved to another
workspace is. What the window does once emptied belongs to
[#21][issue-21].

A window that gains an attachment, including the smoke fixture's, runs its
own full reconcile. Attaching emits no hierarchy event, so nothing else
would visit it.

### Desktop: exit close has one actor (PR 2)

`Desktop` keeps an **owed exit-close** per tab. The first window whose view
observes the tab's exit transition while its config has `close_on_exit` set
records it, as `ExitQueue::observe` decides today. A reload therefore
still affects only future exits, and a tab retained by
`close_on_exit = false` is never closed later.

An owed close moves through three states:

1. **Owed:** any window showing the tab may claim it, however late its view
   was installed. The first to do so queues the close in its `ExitQueue`;
   other windows do not.
2. **Claimed:** the claiming window holds it until its close operation
   starts. If the window drops its view or closes first, the close returns
   to owed, and the window visits the others showing the tab through
   `cx.defer` and `update_windows`, as `reconcile` does, never reading
   another window's entity from inside its own update.
3. **Submitted:** once the assessment or commit has started on a worker,
   the operation owns the close until its worker returns, even if its view
   is dropped or its window closes, so a second window can never assess or
   commit the same close. An application-level completion keyed by tab,
   independent of the window, records the worker's result:
   - success clears the close, and `TabClosed` lets reconcile drop the
     other windows' views;
   - a failure clears it with the failure notice and is not retried, as
     today;
   - an assessment whose window closed before it could start the commit
     reached nothing in Mux, so the close returns to owed for the remaining
     windows. A commit that started always runs to its result, because the
     worker does not depend on the window.

Exit notices stay per window, based on the viewer's exit transition, so a
view installed after the exit announces nothing. Failure notices stay per
window too.

### Smoke fixture (PR 2)

The palette smoke fixture gains `attach-window\t<index>`. It opens a window
without a shell (`launch_shell = false`), attaches it to the session of the
window at `index` through `Mux::attach_session`, records the attachment,
session, and workspace in the window model, and runs that window's full
reconcile, which installs the views. Production code gains no command for
this.

A new `check-viewers.ts` runs inside the macOS and Linux palette smokes, as
`check-overlays.ts` does (see Verification).

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
5. Documentation: AGENTS.md, CONTEXT.md, and the plan references below.
6. `pty::spawn` serializes PTY creation with a process-wide lock.
   Concurrent `openpty` calls in one process failed on macOS 27 under the
   new parallel PTY tests. Mux already serializes production spawns, so
   the lock costs nothing there.

### PR 2: desktop multi-viewer ([#206][issue-206])

1. Split requested and displayed grids in `TerminalView`.
2. Add the install, in-place replacement, and removal rules to reconcile
   and `WorkspaceView`.
3. Add owed exit-closes and claims.
4. Add the smoke fixture and `check-viewers.ts`, and wire them into the
   macOS and Linux palette smokes.
5. Give `ScrollController` a way to clear its latched failure, or skip it
   for `Revoked`, so an in-place viewer replacement resumes snapshots.

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
smokes. A view-level harness lands with PR 2, which needs it for reconcile
and replacement.

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

Moved to PR 2's view-level harness: the refresh applying each status
change exactly once, a view installed after exit reporting no transition,
an exited tab reflowing through its view, and a focus gain, key, and blur
draining through a real viewer as `ESC [ I`, the key's bytes, and
`ESC [ O`.

### PR 2

- Displayed-grid unit tests: mouse positions, page size, and selection
  points against a canonical grid larger and smaller than the requested one;
  a press inside the grid and a release outside the window still send the
  release, after which another viewer's press is accepted.
- Reconcile tests in `windows/projection.rs`: a projected tab without a view
  yields one install; an install in flight, a busy window, or a terminating
  runtime yields none; reconciling twice yields no duplicate; a tab removed,
  or whose viewer was revoked, before the completion is not pushed; a
  revoked view is re-subscribed in place, keeping the active tab, focus,
  and selection, and its focused replacement writes `ESC [ I` and regains
  control; after replacement, new output, a title change, an exit, and a
  clipboard write each reach the view through its restarted activity task
  without display frames; a tab whose workspace moved to another session is
  removed.
- Exit-close tests, with both windows observing the exit before either close
  starts:
  - exactly one close is assessed;
  - the claim returns to owed when the claiming window drops its view before
    starting, and a view installed after the exit takes it over;
  - with the assessment held and then the commit held, dropping the claiming
    view starts no second operation;
  - an assessment failure and a commit failure each clear the close with one
    notice;
  - closing the claiming window after its commit starts and before the
    commit returns leads to one close and no stale-close notice in the other
    window; closing it during the assessment returns the close to owed, and
    the other window then closes the tab;
  - a tab that exited under `close_on_exit = false` stays open after a
    reload enables it, as the existing reload test requires;
  - a view installed after the exit announces nothing.
- `check-viewers.ts` in `smoke:macos-palette` and `smoke:linux-palette`.
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
- Write OSC 52 from the shared terminal and assert from smoke state that
  exactly one window delivered it. The core test owns the exact-once
  guarantee across handoff; the smoke proves the desktop wiring.

### Efficiency gates

Run before and after PR 1, and again for PR 2:

- `mise run ci:benchmarks`: output latency within the 5 ms applied budget,
  at most 1.5 snapshots per key, scroll and link budgets unchanged. The
  publication lock and the runtime's return to live must stay within these.
- `mise run bench:idle` on macOS with an unlocked, unoccluded screen: idle
  wakeups and CPU unchanged.
- A core test floods a terminal with a re-sent unchanged title and asserts
  that a hidden viewer receives no wake after its first.
- For PR 2, report snapshot builds and cache reuses for scrolling and for
  an output burst with one and two visible viewers, from a debug counter in
  smoke state. This is a report, not a gate.

Commands: `mise run check`, `mise run test`,
`mise run test:ghostty-releasesafe`, the macOS host smokes (never the Tart
VM smokes), and `mise run linux:smoke`, cleaning up Docker volumes and images
afterwards. Run `mise run verify` before handoff.

## Documentation

- `AGENTS.md`: replace the single-consumer event and activity rules with the
  viewer rules: per-viewer rearm at snapshot construction, status as state,
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

## Unresolved questions

- None blocking. Mouse arbitration may change after experiments, within
  `MouseArbiter`, and the stale-display wheel limitation above is accepted
  unless the maintainer objects.

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
