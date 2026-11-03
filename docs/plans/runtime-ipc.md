# Local runtime IPC

Status: proposed for [issue #44][issue-44].

Named server mode moves the runtime out of the desktop process. This plan
defines the contract that carries runtime requests, results, the hierarchy
stream, terminal viewers, terminal updates, input, and client-routed host
effects across a local process boundary. It also sets the bounds, retry
rules, and transport that keep the contract safe. Named server lifecycle and
GUI attachment ([#45][issue-45]), the CLI ([#46][issue-46]), and the browser
gateway ([#133][issue-133]) build on it and are not planned here.

Parent epic: [#18][issue-18]. Dependencies [#22][issue-22] (viewers),
[#23][issue-23] (command catalog), [#98][issue-98] (hierarchy projection),
and [#80][issue-80] (OSC 52 writes) have landed. The
[hierarchy projection plan](hierarchy-projection.md) and the
[terminal viewers plan](terminal-viewers.md) each end with "Contract items
for #44"; this plan settles every one of them.

## Settled decisions

Agreed with the maintainer on 2026-11-03, before drafting:

- **Additive evolution within a major version.** A newer GUI or CLI keeps
  working with an older running daemon of the same major version, so a
  Sparkle update does not force a daemon restart that would end its shells.
  Only a major version bump requires a restart.
- **A hand-written codec in a new `huterm-wire` crate.** `huterm-wire`
  depends only on `huterm-protocol`, and `huterm-protocol` keeps its empty
  dependency set. The architecture gate gains rules; none is weakened.
- **At-most-once retry.** After a broken connection, the client retries only
  queries, set-to-value requests, and input the server refused before
  admitting it. Every other in-flight request reports an unknown outcome,
  and the resynchronized state shows what happened.
- **proptest and cargo-fuzz.** Property tests run on the pinned stable
  toolchain in the normal suite. cargo-fuzz targets run on a pinned nightly
  through a Mise task and a scheduled CI job.
- **Embedded mode stays in-process.** Production embedded mode never
  serializes. Contract tests run the same operations through the in-process
  API and through the IPC server and client, and compare the results.
- **Socket placement.** Linux uses `$XDG_RUNTIME_DIR/huterm/` when it is set
  and owned by the user, else `/tmp/huterm-<uid>/`. macOS uses
  `/tmp/huterm-<uid>/`, as tmux does. `HUTERM_SOCKET_DIR` overrides the
  directory.

## Current state

Observed on `main` at `56cbe26e`:

- `huterm-protocol` has no dependencies; `mise run architecture` fails if it
  gains a normal or build dependency. The workspace already pins `serde`,
  but only `huterm-config` and `huterm-gpui` use it.
- `RuntimeId` is allocated from a process-global counter in
  `crates/huterm-core/src/mux.rs` (`NEXT_RUNTIME`). `SessionId`,
  `WorkspaceId`, `TabId`, `PaneId`, `SplitId`, `AttachmentId`, and
  `ViewerId` carry it. `TerminalId` is an unscoped `opaque_id`, though Mux
  allocates it from the same per-runtime counter as the scoped IDs.
- `StreamId` wraps a `RuntimeId`, and its documentation reserves room for a
  server incarnation. The reducer rejects an envelope from another stream.
- Several protocol types are not wire-ready as they stand:
  `TerminalSnapshot.rows` is `Vec<Arc<TerminalRow>>`; `TerminalCommand`
  holds `PathBuf` values; `Viewport.bottom_offset`, `BufferPoint`, and
  `TerminalSnapshot.history_size` are `usize`; `CommandId` wraps a
  `&'static str` that only a catalog lookup can produce; `ClipboardWrite`,
  `TerminalDirectory`, `ScrollbackLimit`, and `PaneLayout` keep invariants
  behind constructors.
- A viewer learns of changes through `TerminalViewer::poll` and builds a
  complete snapshot on request. The runtime rearms a viewer's notification
  only when it builds that viewer's snapshot, so a viewer that requests
  nothing gets one wake and no further work.
- The desktop retries input refused with `RuntimeError::Busy` on a 16 ms
  timer that is armed only while work is pending
  (`crates/huterm-gpui/src/desktop.rs`). All viewers of a terminal share one
  1 MiB input byte budget (`INPUT_BYTE_CAPACITY`).
- Revoking a viewer discards its queued requests, even when the viewer was
  dropped first (`Registry::revoke_where` and `RuntimeMessage::permitted`).
  `detach_session` revokes every viewer of the attachment.
- `HostEffectSink` treats `LocalText` and `Remote` recipients as ineligible
  and selects one recipient per effect by activity ordinal, never
  redirecting. A terminal holds at most 8 queued effects and 64 MiB of text.
- `DesktopRuntime` (`crates/huterm-gpui/src/desktop/windows.rs`) is the
  desktop's only path to Mux: spawn, split, layout, close tickets, capture,
  catalog commands, reorder, and terminate, all run behind its worker.
- Layout undo restores core-produced `LayoutChange` records. The core
  runtime guide warns that a client able to forge records over IPC needs a
  stronger check.

## Scope

In scope:

- The wire contract: preamble, framing, codec, messages, version and
  feature negotiation, server incarnation, errors, and limits.
- Protocol changes the contract needs: a scoped `TerminalId`, a shared
  `RuntimeId` allocator, `ServerIncarnation`, and a `StreamId` that carries
  it.
- Terminal updates as deltas against a per-viewer baseline, and status as
  state.
- A server library that serves one connection against a `Mux`, and a client
  library that speaks the contract, both testable over in-memory streams.
- Unix domain socket placement, permissions, and peer authentication.
- Contract tests, golden vectors, property tests, fuzz targets, and a
  `wasm32-unknown-unknown` build gate.

Out of scope, with the issue that owns each:

| Deferred work | Owner |
| --- | --- |
| The daemon binary, start, stop, status, startup races, stale sockets, supervision | [#45][issue-45] |
| The desktop's server mode, session picker, reconnect UI, and Quit that detaches | [#45][issue-45], [#26][issue-26] |
| Which side fills shell, directory, and environment defaults for spawns in server mode | [#45][issue-45] |
| `close_on_exit` as runtime policy and server-mode close prompts | [#21][issue-21], [#45][issue-45] |
| Attachment events in the hierarchy stream | [#26][issue-26], [#21][issue-21] |
| CLI commands, output retrieval, exit codes | [#46][issue-46] |
| Server-owned persistence and daemon restart restore | [#42][issue-42], [#45][issue-45] |
| Rich content capabilities and transfers | [#88][issue-88] |
| WebSocket framing, browser authentication, gateway | [#131][issue-131], [#133][issue-133] |
| Browser and TUI clients | [#134][issue-134], [#48][issue-48] |
| A client-facing service trait shared by native and browser presentation | [#132][issue-132] |

## Principles

The [hierarchy projection principles](hierarchy-projection.md#principles)
apply: reactive and never polled, idle costs nothing, work scales with the
change, correctness never depends on observing a particular event, and loss
is detected. This plan adds:

- **The runtime never waits for a client.** PTY reading, parsing, and other
  viewers continue whatever one connection does. A slow client receives
  fewer, larger updates; a stalled one is disconnected.
- **State over logs.** Terminal content and status travel as state against
  a baseline, so recovery costs at most one complete snapshot and the server
  keeps no per-client damage log.
- **Every bound is a named constant,** enforced before allocation, with a
  test at the boundary.
- **The server repeats every check.** Capabilities, revocation, ID scope,
  and argument validation run on the server per message, so enforcement
  never depends on client code.
- **The contract is sans-IO.** Codec, framing, negotiation, and update
  application are pure functions over bytes and values. Sockets and threads
  live in thin native adapters, so a browser can drive the same code from
  WebSocket callbacks.

## Shape

```text
huterm-protocol   IDs, events, snapshots, reducer          (no dependencies)
      ^
huterm-wire       preamble, frames, codec, messages,       (protocol only;
      ^           negotiation, update encoder/decoder,      wasm32)
      |           golden vectors
      +------------------------------+
huterm-client                        huterm-server
  sans-IO client session (wasm32)      listener, socket path, peer auth,
  native Unix transport (cfg unix)     connection handler over a Mux
      ^                                    ^
  GUI server mode (#45), CLI (#46),    daemon binary (#45)
  browser adapter (#134)               huterm-core
```

Architecture gates added to `mise run architecture`:

- `huterm-wire`'s normal and build dependencies are exactly
  `huterm-protocol`.
- `huterm-client` depends on no `huterm-core`, `huterm-ghostty`,
  `portable-pty`, or `gpui-pre`, on any target.
- `huterm-server` depends on no `gpui-pre`.
- `huterm-protocol`, `huterm-wire`, and `huterm-client` without its native
  transport build for `wasm32-unknown-unknown`, including their test
  targets. `rust-toolchain.toml` gains that target, and
  `scripts/policy-alignment.test.ts` keeps `mise.toml` aligned with it.

The existing empty-dependency rule for `huterm-protocol` is unchanged.

## Decisions

### Crates and the dependency policy

Serialization stays out of `huterm-protocol`. A derive there would turn
every in-process type change into a wire change, would bypass constructor
validation unless each type gained a shim, and, with a compact
non-self-describing format such as postcard, would force lockstep versions.

`huterm-wire` encodes and decodes protocol types directly with explicit
functions. Encoders destructure each struct exhaustively, so adding a field
to a protocol type fails to compile until someone decides its wire
representation. Decoders build values through the protocol's validating
constructors, so a peer can never produce a `PaneLayout` or
`ScrollbackLimit` that core could not.

Two more crates hold the code that is not the contract itself:

- `huterm-client` holds the client session state machine (handshake, request
  table, incarnation mapping, hierarchy and viewer streams, input
  sequencing, credit) as a sans-IO type, and a native transport that runs it
  over a `UnixStream` with reader and writer threads and exposes async
  handles through `async-channel`, which core already uses. The CLI blocks
  on those handles; GPUI awaits them.
- `huterm-server` holds the listener, socket path rules, peer
  authentication, and the per-connection handler that maps messages onto
  `Mux` and `TerminalViewer` calls.

Rejected:

- **`serde` derives in `huterm-protocol` with postcard.** Least code, but it
  couples internals to the wire and gives up the version-skew decision
  above.
- **Mirror wire types with serde and postcard.** Decoupled, but it doubles
  the type surface and conversion code, and still needs versioned duplicate
  types to evolve because postcard cannot skip fields.
- **Protocol Buffers through `prost`.** Solves field evolution but adds a
  schema compiler or `protox` to the build and still converts generated
  types to protocol types.
- **One crate for client and server.** The browser must build the client
  without core.

### Codec and framing

Each side opens a connection by writing an 8-byte preamble: the bytes
`HUTRM`, a zero byte, and a little-endian `u16` framing version, which starts
at 1. The preamble never changes, so a peer rejects a foreign or
future-framing endpoint before it parses a frame.

After the preamble, the stream is a sequence of frames: a little-endian
`u32` payload length, then the payload. A zero length, or a length above the
current frame limit, is a protocol violation that closes the connection,
because a byte stream cannot resynchronize after a bad length. The frame
limit is `HANDSHAKE_FRAME_LIMIT` (16 KiB) until the handshake completes, then
the negotiated `max_frame`: the smaller of both peers' values, 1 MiB by
default and never below 64 KiB.

A payload is a message kind, as a varint, followed by that message's record.
The primitives:

| Value | Encoding |
| --- | --- |
| Unsigned integers | LEB128 varint, range-checked against the target width |
| Signed integers | Zigzag varint |
| `bool` | One byte, 0 or 1; any other byte is invalid |
| Fixed arrays (`Rgb`, incarnation) | Raw bytes |
| Strings | Varint length, then UTF-8, checked against a per-field maximum before allocation |
| Byte strings and paths | Varint length, then bytes, with a per-field maximum |
| Sequences | Varint count, checked against a per-field maximum before allocation, then items |
| `Option` | Tag byte 0 or 1, then the value |
| Struct | Varint byte length, then fields in declared order |
| Enum | Varint tag, then the variant's record |

Struct records make additive evolution cheap. A decoder reads the fields it
knows and skips any bytes left in the record, which belong to fields a newer
peer appended. A record that ends before a field the decoder knows is valid
only if that field was added in a later minor version, and the field then
takes its declared default. Fields are only ever appended; removing or
reordering one is a major version change.

Enum tags and message kinds never get skipped. A sender never sends a
variant or message the peer did not negotiate (see the next section), so an
unknown tag is a protocol violation that closes the connection.

Platform types get explicit wire forms. `usize` fields travel as `u64` and
fail to decode when they do not fit the receiving target. Paths travel as
bytes: on Unix the encoder takes `OsStr` bytes, and on other targets
encoding and decoding require UTF-8. A `CommandId` travels as its string, and
decoding resolves it through the catalog; an unknown ID decodes to a request
the server answers with `UnknownCommand`, carrying the text.

Terminal rows use a dedicated packed form, described under terminal
updates, because the generic struct form would cost a length prefix per
cell.

Payloads larger than one frame (selection text, clipboard writes, pastes,
and large terminal updates) travel as a sequence of frames under one
transfer, with a final-frame flag. The receiver checks the declared total
against the per-kind cap before buffering and applies nothing until the
final frame arrives. Frames of other transfers and messages may interleave,
so a large snapshot cannot hold up a keystroke echo in another terminal.

### Versions, features, and server incarnation

The client sends `Hello` first:

- the protocol major version and the highest minor version it speaks;
- its feature set, as numeric feature IDs registered in `huterm-wire`;
- its client kind: desktop, text, gateway, or command line;
- its `max_frame`, its name, and its build version, for diagnostics.

The server answers with `Welcome` or `Reject`, then closes after a
`Reject`. `Welcome` carries:

- the major version, and the negotiated minor version: the smaller of the
  two peers' minor versions;
- the negotiated features: the intersection of both sets;
- the server incarnation, the socket name, the server's build version, and
  its process ID;
- the negotiated `max_frame` and the limits that bind the client, such as
  credit and in-flight request maximums.

`Reject` carries a reason code (`IncompatibleMajor` with the server's
versions, `ServerBusy`, or `Malformed`) and a diagnostic message. Any
message before `Hello` other than `Hello` itself, or no `Hello` within
`HANDSHAKE_DEADLINE` (5 s), closes the connection.

A minor version may only add messages, enum variants, and appended fields
that a peer on an older minor version can do without. Senders withhold
anything newer than the negotiated minor version. A change an older peer
cannot do without is a major version change. Features gate optional
behavior inside a minor version: rich content ([#88][issue-88]) is the first
reserved feature, and attachment events ([#26][issue-26]) are expected to
arrive as a minor version. A client that lacks a feature still receives
complete text-terminal operation.

Golden vectors pin the encoding: `crates/huterm-wire/tests/vectors/` holds
canonical values and their bytes per minor version. A test fails if an
existing vector's bytes change, so a released minor version cannot change
silently. A new minor version adds vectors.

The client kind is policy, not authentication: every peer already passed
the same-user check. It selects host-effect eligibility and the defaults a
client gets.

**Server incarnation.** `huterm-protocol` gains `ServerIncarnation`, 16
bytes the server reads from `/dev/urandom` at startup. It distinguishes
server processes across restarts and machines. IDs on the wire are bare
`u64` values; the connection's incarnation scopes them. That keeps every
message small and makes scope a property of the connection rather than of
each ID.

- `RuntimeId` allocation moves from core's `NEXT_RUNTIME` to
  `RuntimeId::allocate` in `huterm-protocol`, a process-wide atomic counter,
  so an embedded Mux and remote connections in one process never share a
  `RuntimeId`.
- The client maps each incarnation it meets to a client-local `RuntimeId`
  and decodes every ID from that connection in that scope. Reconnecting to
  the same incarnation reuses the mapping, so IDs held by windows stay
  valid. A different incarnation gets a new `RuntimeId`, so every old ID is
  foreign and the existing scope checks reject it.
- Encoding an ID whose `RuntimeId` is not the connection's fails locally
  with `StaleTarget`; it is never sent.
- `TerminalId` becomes a scoped ID. Today a stale terminal ID held across a
  daemon restart would name an unrelated live terminal, because restored
  and fresh runtimes reuse small numbers.
- `StreamId` gains `incarnation: Option<ServerIncarnation>`. Embedded
  streams keep `None`; remote streams carry the connection's incarnation,
  which the client fills in. The wire sends the incarnation once, in
  `Welcome`, not per event. A changed incarnation is a stream change, which
  cancels sequence waiters as the hierarchy plan specifies.

### Requests, results, and errors

The client numbers requests with a per-connection `RequestId`, starting at 1
and never reused. The server answers each request exactly once with a
`Result` carrying the same ID, unless the connection ends first. Results may
arrive in any order. At most `MAX_IN_FLIGHT_REQUESTS` (64) requests may be
outstanding per connection; past that the server answers `Busy` without
acting. `Cancel { request }` asks the server to abandon a request that has
not committed. The request still gets exactly one result.

A result is a success value or a `WireError`:

- `code`: a stable numeric error code, extensible by minor version, mapped
  from `CommandError`, `MuxError`, and `RuntimeError`. Examples:
  `StaleTarget`, `ForeignScope`, `NotPermitted`, `Revoked`, `Stopped`,
  `Busy`, `Unsupported`, `InvalidArgument`, `LimitExceeded`, `StaleClose`.
- `retry`: `Safe` when nothing was committed and the same request may be
  sent again, such as `Busy`; `Never` otherwise.
- `committed_seq`: the hierarchy sequence for a structural request that
  committed before failing, as a failed close cleanup does today.
- `message`: a diagnostic string, never parsed by clients.

Every structural result carries the hierarchy sequence it committed at, so
each client keeps read-your-writes through the existing sequence waiters.
Over IPC a result and its events can arrive in either order, which those
waiters already handle.

The request set mirrors the operations the desktop reaches through
`DesktopRuntime` and `TerminalViewer` today, plus catalog commands. Slice 3
adds the inventory. Two kinds of state stay on the server instead of
crossing the wire as values:

- **Close tickets.** `prepare_close` returns a ticket ID bound to the
  connection, at most 16 per connection. Check and commit name the ticket.
  The existing revision check still turns a changed hierarchy into
  `StaleClose`, and a disconnect discards the connection's tickets.
- **Layout history.** A rearrangement result returns an opaque undo token
  bound to the connection, and the server keeps the `LayoutChange` it names.
  The client keeps the last token each way per window, as layout history
  does today. A client cannot forge a record, and a disconnect discards
  the tokens, so undo is unavailable after reconnect.

### Retry and reconnect

Every request kind has a static retry class:

| Class | Examples | After a broken connection |
| --- | --- | --- |
| Query | Snapshot, selection text, link lookup, list or select targets | Retried |
| Set-to-value | Rename to a name, report geometry, set focus, submit presentation, set scrollback, set input sync, resize split, set zoom | Retried |
| Mutation | Spawn, split, close, move, arrange, reorder, clear history, reset, input | Never retried; reports `OutcomeUnknown` |

After reconnecting, the client resubscribes to the hierarchy, installs the
new snapshot, and then reports an unknown outcome to each waiting mutation.
Its effect, if it committed, is visible in that snapshot, and the
level-triggered reconcile installs or removes views from state. A tab whose
spawn result was lost therefore appears as a tab the window did not spawn,
which reconcile already handles.

Input admitted by the server is never resent, so a keystroke typed during a
disconnect can be lost but never duplicated. Input the server refused before
admission is resent in order (see input below). Host effects are never
resent or replayed.

Rejected: idempotency keys with a server-side dedupe table. Retrying a create
or close exactly once would need a client identity that outlives a
connection, plus retention and eviction rules, and the resynchronized state
already shows each outcome.

### Hierarchy over the wire

`SubscribeHierarchy` returns the current `HierarchyState`, its sequence,
and its digest. The server then forwards each envelope as a
`HierarchyEvent { seq, event }` message. The connection supplies the stream,
so the client rebuilds the `StreamId`.

The server drains its core `HierarchySubscription` only while the
connection's hierarchy lane is under its byte budget. When the client reads
slowly, core's bounded queue lags that subscription, as designed. Once the
lane drains, the server sends `HierarchyResync` with a fresh snapshot in
place of the discarded events, and the client installs it through the
existing resync path. The server resynchronizes on its own because it is
the side that observed the lag, and it saves a round trip.

Digests follow the hierarchy plan's contract item: `{seq, digest}` rides on
every snapshot, every structural result, and every 256th event
(`HIERARCHY_DIGEST_INTERVAL`). A client whose projection digest differs at
that sequence sends `ResyncHierarchy`. `HierarchyState::digest()` already
exists; this plan freezes its algorithm, 64-bit FNV-1a over the canonical
wire encoding of the state, and pins it with a golden vector.

A hung server keeps its socket open, so the client arms a one-shot deadline
only while a request or sequence waiter is outstanding, as the hierarchy
plan specifies. An idle connection arms nothing.

### Viewers and terminal updates

A remote viewer is a `TerminalViewer` the server holds for the connection.
`SubscribeTerminal { attachment, terminal, options }` creates it with the
same validation as in process and returns its `ViewerId`, the current
`TerminalStatus`, and a complete first update. The server checks the
requested capabilities against the client kind, and a disconnect drops every
viewer the connection holds.

**Engine damage, publication, and wire updates stay separate.** Core keeps
consuming engine damage when it builds a snapshot, under its own publication
ownership, exactly as today. The server turns snapshots into wire updates
per viewer. No client message resets engine damage, and no update needs an
acknowledgement to stay correct.

**The baseline is the last update sent.** For each remote viewer, the server
keeps the `Arc<TerminalSnapshot>` it last encoded for that client. Its rows
are mostly the same `Arc<TerminalRow>` values as the current snapshot, so the
baseline costs at most one viewport of rows per viewer, with no damage log.
To build an update, the server compares each current row with the baseline:

- A row whose `Arc` is identical to a baseline row encodes as
  `Keep(index)`, naming the row's position in the baseline. Scrolling
  therefore costs a few bytes per row.
- Any other row encodes in full. A row rebuilt with unchanged content costs
  bytes but never correctness.

Pointer identity is used only on the server, between two snapshots it holds.
The wire carries baseline row positions, which mean the same thing to both
ends, so nothing depends on identity surviving serialization. Because the
diff runs against the last update sent rather than the previous publication,
every skipped or coalesced publication is folded into the next update.

**Complete snapshots.** An update has no baseline, and every row is in full,
when the viewer is new or reconnected, when the geometry revision changed
since the baseline, when the client sent `ResyncViewer`, or when the server
dropped its baseline under a memory bound. Recovery is therefore always one
complete snapshot.

**Every update carries every non-row field** of the snapshot: content
generation, publication revision, geometry revision, grid size, cursor,
modes, viewport, history size, and cursor color. Dynamic palette colors
reach clients as resolved cell colors and cursor color, as they do in
process. A metadata-only change, such as cursor movement, a mode switch, or
a viewport move with no new rows, is an update with only `Keep` row
operations.

**Row encoding.** An update carries a style table of the distinct
`(foreground, background, CellStyle)` values its full rows use. A full row is
a sequence of runs, each a style index, a cell count, and the cells' texts.
Each `CellText` is limited to `MAX_WIRE_CELL_TEXT` (1 KiB). The decoder
checks that every row has exactly `size.columns` cells, that there are
exactly `size.rows` rows, that every `Keep` index exists in its baseline, and
that every style index exists. An update that fails any check is a protocol
violation.

**Digests.** Every complete snapshot, and every 64th delta
(`UPDATE_DIGEST_INTERVAL`), carries a 64-bit FNV-1a digest of the canonical
encoding of the resulting rows and metadata. A client whose result differs
sends `ResyncViewer`. The digest is computed only at those points, so steady
state pays nothing for it.

**Credit paces updates.** The client grants each viewer credit, at most
`MAX_VIEWER_CREDIT` (2) outstanding updates, and returns a credit after it
applies an update. The server's pump for a viewer waits on the viewer's
wake; when the viewer is notified and has credit, it requests a snapshot,
encodes the update, and spends a credit. A client grants no credit to a
hidden tab, so its viewer stays notified, the runtime does not wake it again,
and a hidden terminal costs the wire nothing until it is shown. This is the
in-process rearm rule carried across the wire.

**Status is state.** When a viewer's status revision changes, the server
puts the whole `TerminalStatus` in the viewer's single pending status slot,
replacing any unsent one, and sends it ahead of that viewer's next update.
Status does not spend credit. The client derives exits, new bells, and missed
failures from its own baseline. The derivation in `TerminalViewer::poll`
moves into a `StatusBaseline` type in `huterm-protocol` so both clients share
one implementation.

**Scrolling.** A scroll request is answered through the viewer's pump: the
pump applies `request_scrolled_snapshot`, sends the resulting update without
spending credit, then sends the request's result. Scroll numbers stay
assigned by the client, so the return-to-live rule needs no terminal-wide
counter.

**Revocation and stop.** The server sends `ViewerRevoked` or
`ViewerStopped` once and stops the pump. A revoked viewer's requests answer
`Revoked`. A stopped terminal's final status and last update remain the
client's to show.

### Input

Input is not request-and-result. Each viewer numbers its input with a
client-assigned `InputSeq`, and the server admits input from one viewer in
that order.

- Core gives each remote viewer its own input byte budget,
  `VIEWER_INPUT_BUDGET` (1 MiB), passed through `ViewerOptions`. One
  connection's paste then cannot refuse another connection's keystrokes, as
  the viewers plan requires. Embedded viewers keep today's shared
  per-terminal budget.
- When core answers `Busy`, the server holds that viewer's input in an
  ordered staging queue, bounded by the same budget, and retries on the
  16 ms timer the desktop uses, armed only while input is staged.
- When the staging queue is full, the server refuses that input with
  `InputRefused { viewer, seq }` and refuses every later input from the
  viewer until the client sends `ResumeInput { viewer, seq }`. Ordering
  therefore holds across a refusal. The server sends `InputCapacity` when
  staging falls below half, and the client resends from the refused
  sequence. Refused input never reached core, so resending it is safe.
- A paste larger than one frame is a chunked transfer that the server
  reassembles into one `TerminalInput::Paste` before admission, so
  bracketed paste stays atomic.
- Focus and geometry reports keep their in-process rules: focus is ordered
  with input and outside the byte budget, and geometry is latest-wins.

### Disconnect

A broken connection, a protocol violation, or a stalled writer ends the
connection the same way:

1. The server drops the connection's viewers. Their admitted input still
   runs; then core finalizes them, releasing held mouse buttons and counting
   focus out.
2. The server detaches the connection's attachments. Detaching must not
   revoke a viewer that is already dropped, or its admitted input would be
   discarded. Slice 3b changes `Registry::revoke_where` callers on the detach
   path to skip dropped slots, with a test that input admitted before a
   disconnect reaches the PTY.
3. The server discards the connection's close tickets, undo tokens, staged
   input, and host-effect recipients.

A disconnect never closes a terminal, workspace, or session.

### Bounds and backpressure

| Bound | Starting value | Exceeded |
| --- | --- | --- |
| `HANDSHAKE_FRAME_LIMIT` | 16 KiB | Close |
| `max_frame` | 1 MiB, at least 64 KiB | Close |
| `HANDSHAKE_DEADLINE` | 5 s | Close |
| `MAX_CONNECTIONS` per server | 64 | `Reject { ServerBusy }` |
| `MAX_IN_FLIGHT_REQUESTS` per connection | 64 | `Busy`, retry safe |
| `MAX_CONNECTION_VIEWERS` | 256 | `LimitExceeded` |
| Viewers per terminal | 32, as today | `LimitExceeded` |
| `MAX_VIEWER_CREDIT` | 2 | Protocol violation |
| `VIEWER_INPUT_BUDGET` | 1 MiB | Staged, then `InputRefused` |
| Hierarchy lane budget per connection | 1 MiB | Lag, then `HierarchyResync` |
| `OUTBOUND_BUDGET` per connection | 8 MiB | Writer stall clock starts |
| `WRITER_STALL_DEADLINE` | 30 s | Disconnect |
| Close tickets per connection | 16 | `LimitExceeded` |
| Transfer caps | Clipboard 64 MiB, selection 64 MiB, paste = input budget | `LimitExceeded` |

Every outbound source has its own bound, so the outbound queue cannot grow
without limit: results are capped by in-flight requests, updates by credit,
status by one slot per viewer, hierarchy events by the lane budget and lag,
and host effects by their budgets. When the socket accepts no bytes and the
queue is over `OUTBOUND_BUDGET` for `WRITER_STALL_DEADLINE`, the server
disconnects that client. The deadline is armed only while the queue is over
budget.

The values are starting points, named in `huterm-wire` or `huterm-server`,
and each has a boundary test. A client that sends more than the negotiated
limits is disconnected; a server never sends more than it advertised.

PTY output is never lost to a slow client. The runtime never waits on the
server, the server never waits on a socket while holding a runtime or Mux
lock, and a slow viewer's next update folds in everything it missed.

### Client-routed host effects

A viewer subscribed with `host_effects` registers a recipient whose origin
follows the client kind:

- A desktop client on local IPC registers as `LocalDesktop`, which replaces
  `LocalEmbeddedDesktop` and is eligible as the embedded desktop is today.
  Without this, OSC 52 would stop working in server mode.
- A text or command-line client registers as `LocalText`, and a gateway as
  `Remote`. Both stay ineligible until [#48][issue-48] and
  [#131][issue-131] define their policies.

Selection is unchanged: one eligible recipient by activity ordinal, chosen
at admission, never redirected. The server pops a delivery only when the
connection's host-effect lane is free, rechecks `is_current`, assigns a
connection-scoped `EffectId`, and sends `HostEffect { effect, terminal,
kind, payload, response }`. Large payloads use a chunked transfer.

- **No replay.** Effect IDs belong to one connection. A disconnect revokes
  the connection's recipients, which clears their queues, and nothing is
  ever re-sent on a later connection. A client ignores effect frames it
  cannot match to its current connection.
- **Delivery deadline.** An effect not written to the socket within
  `HOST_EFFECT_DELIVERY_DEADLINE` (2 s) after admission is dropped. A
  stalled remote recipient therefore cannot hold the terminal's budget of 8
  effects and deny every other recipient, which the viewers plan flagged.
  Each remote recipient also gets its own effect budget inside the
  terminal's.
- **Response correlation.** An effect that needs an answer carries a
  response deadline. The server keeps a bounded pending entry per effect and
  answers the terminal with the effect's documented fallback on timeout,
  disconnect, or a refusal. A late or unknown `HostEffectResult` is ignored.
  OSC 52 writes need no response; their fallback for a missing or
  incapable client is to drop the write, as in process.
- Terminal processing never waits on any of this.

### Transport and authentication

Named servers listen on Unix domain sockets only. No code in this plan opens
a network listener.

Socket path resolution, in order:

1. An explicit path, which tests and `--socket` style flags use.
2. `HUTERM_SOCKET_DIR/<name>`.
3. Linux: `$XDG_RUNTIME_DIR/huterm/<name>` when `XDG_RUNTIME_DIR` is set,
   absolute, and owned by the effective user.
4. Otherwise: `/tmp/huterm-<uid>/<name>`, on macOS and Linux alike. A GUI
   launched by launchd and a CLI over ssh resolve the same path without
   depending on `$TMPDIR`.

`<name>` defaults to `default`, as the
[workspace plan](workspaces-windows-tabs.md) specifies, and must match
`[A-Za-z0-9][A-Za-z0-9._-]{0,63}`. The full path must fit `sun_path` (104
bytes on macOS, 108 on Linux), or resolution fails with a clear error rather
than truncating.

The server creates the socket directory with mode `0700` and checks it with
`lstat`: it must be a real directory, owned by the effective user, with no
group or other permissions. The server refuses to listen otherwise, as tmux
does. The socket itself is created with mode `0600` under a restrictive
umask. These checks belong to `huterm-server` in this plan; stale-socket
detection and startup races belong to [#45][issue-45].

After `accept`, the server reads the peer's credentials, through
`SO_PEERCRED` on Linux and `getpeereid` on macOS (both through the pinned
`nix`), and closes any connection whose user ID differs from its own
effective user ID, before the preamble. The client checks the directory the
same way before connecting, so it never talks to a socket another user
planted.

The server serves each connection with a reader thread, a writer thread,
and one executor thread for its pumps and request handling. Mux-locking and
blocking work (spawn, close, assessment) runs on worker threads, never on a
connection's I/O or pump threads. Slice 3 picks the executor from crates
already in `Cargo.lock` and runs the license and release-age checks.

### Embedded and server modes share one contract

Embedded mode keeps its in-process calls, its `Arc`-shared snapshots, and no
endpoint. Server mode puts the codec between the same operations. The
semantics live in core and `huterm-protocol` once, and the wire adds only
transport behavior: framing, correlation, credit, staging, and recovery.

Contract tests prove the two paths agree. A test-only adapter trait in
`huterm-server`'s tests exposes the operations both paths support. One
implementation calls `Mux` and `TerminalViewer` directly. The other runs the
server's connection handler and the client session over a `UnixStream` pair
in one process. Each scripted scenario runs on both, then compares
results, error codes, hierarchy digests, statuses, and viewport rows. The
browser gateway's contract tests ([#133][issue-133]) can reuse the same
scenarios later.

A production client-facing trait for the desktop, shared by native and
browser presentation, belongs to [#132][issue-132] and [#45][issue-45]. This
plan does not add one ahead of a production caller.

## Delivery

Five pull requests for #44, each mergeable on its own. Slices 1 and 2 are
pure contract code; nothing calls them until slice 3.

### Slice 1: identity and the wire foundation

- `huterm-protocol`: `ServerIncarnation`, `RuntimeId::allocate`, a scoped
  `TerminalId`, and `StreamId` with an incarnation. Core and the desktop
  migrate mechanically.
- New `huterm-wire`: preamble, frames, codec primitives, `Hello`,
  `Welcome`, and `Reject` with the negotiation function, `WireError` and
  its codes, ID encoding, and the encodings for hierarchy state, events,
  pane layouts, catalog invocations, and outcomes.
- Golden vectors for minor version 0, the architecture gates, the
  `wasm32-unknown-unknown` check, and the new crate's `deny` and license
  coverage.
- The `fuzz/` directory (outside the workspace) with frame and message
  decoder targets, a `fuzz:wire` Mise task on a pinned dated nightly that
  satisfies the release-age policy, and a scheduled, manually dispatchable
  workflow that is not a required check. The dependencies and CI guides
  record the nightly pin and how to bump it.

Acceptance: every vector encodes to its bytes and decodes back; truncated,
oversized, overlong-varint, invalid-UTF-8, unknown-tag, and bad-bool inputs
fail with a typed error and no panic; negotiation picks the smaller minor
version and the feature intersection and rejects a different major version;
an ID from another `RuntimeId` refuses to encode; the wasm check passes; the
gate fails when a dependency is added to `huterm-wire`.

### Slice 2: terminal updates and status

- `huterm-wire`: the update encoder (baseline, current snapshot, style
  table, `Keep` operations, row ranges per frame) and the decoder that
  applies an update to a client baseline, status and input messages,
  chunked transfers, and digests.
- `huterm-protocol`: `StatusBaseline`, with `TerminalViewer::poll` moved
  onto it.
- A fuzz target that applies arbitrary update sequences to a baseline.

Acceptance: a client that receives any subsequence of a server's updates
for a publication sequence ends with exactly the latest snapshot's rows and
metadata; a geometry change yields a complete snapshot; a metadata-only
change yields an update with no full rows that changes the client's cursor,
modes, or viewport; two snapshots with equal content but distinct row
`Arc`s produce the same client state; a `Keep` index outside the baseline is
rejected.

### Slice 3a: server and client, requests and hierarchy

- New `huterm-server` connection handler and `huterm-client` session over
  in-memory streams: handshake, request table, results and errors, cancel,
  incarnation mapping, hierarchy subscription with lag, resync, and digest
  checks, close tickets, undo tokens, and the request inventory.
- The contract-test harness and its first scenarios: create, rename, move,
  reorder, split, arrange, undo, close with tickets, and stale targets.

Acceptance: each scenario gives the same results and hierarchy digests in
process and over IPC; a forced core lag resynchronizes a slow client
without losing a committed change; a reconnect to a new incarnation makes
old IDs foreign and cancels sequence waiters; a mutation in flight at
disconnect reports `OutcomeUnknown` and is never resent; a query in flight
is retried.

### Slice 3b: viewers, updates, and input over IPC

- Remote viewers, update pumps with credit, status slots, scrolling, input
  sequencing with staging, `InputRefused`, `ResumeInput`, and
  `InputCapacity`, per-viewer input budgets in core, and the disconnect
  order with its core change.
- Contract scenarios for snapshots, scrolling, input, focus, geometry
  control, and revocation.

Acceptance: a fast and a slow client on one terminal converge on the same
rows and metadata while the slow one skips publications; a client that
stops reading stalls neither PTY parsing nor the other viewer, and is
disconnected after `WRITER_STALL_DEADLINE`; one connection's large paste
does not refuse another connection's keystrokes; input admitted before a
disconnect reaches the PTY, and input after it never does; a hidden viewer
without credit causes no updates during output.

### Slice 4: host effects over IPC

- Recipient origins by client kind, the effect lane, delivery deadline,
  per-recipient budgets, response correlation, and disconnect cancellation.

Acceptance: an OSC 52 write reaches exactly one eligible desktop client
across embedded and IPC viewers; a text client is never selected; an effect
pending at disconnect is not delivered on reconnect or to another client; a
recipient that stops reading cannot block later effects for other
recipients; a timed-out response produces the fallback without stalling
terminal output.

### Slice 5: Unix socket transport and authentication

- Path resolution, directory and socket permission checks, peer
  credentials, the listener with `MAX_CONNECTIONS`, the handshake deadline,
  and the native client transport's `connect`.

Acceptance: a socket directory with group or other permissions, a symlinked
directory, or one owned by another user is refused; a name that would
overflow `sun_path` fails with a clear error; a peer with another user ID is
closed before the handshake (Linux in Docker, where a second user is
available; macOS covers the same-user path); partial reads and writes split
at every byte of a frame decode correctly; a client that sends no `Hello` is
closed at the deadline.

### What follows

- [#45][issue-45] adds the daemon binary and its lifecycle on
  `huterm-server`, calls `raise_open_file_limit` first, and moves the
  desktop's `DesktopRuntime` operations behind an interface with an
  embedded and a `huterm-client` implementation. It also adds the server
  half of [#21][issue-21].
- [#46][issue-46] builds the CLI on `huterm-client`, with read-only and
  bounded-output behavior.
- [#133][issue-133] carries `huterm-wire` messages in WebSocket binary
  messages, one message per WebSocket message, with each browser page as its
  own IPC connection.

## Verification

| Concern | Evidence |
| --- | --- |
| Fast and slow clients converge; resync is bounded | Slice 2 property test over random publication and skip sequences; slice 3b two-client test against a live PTY |
| Serialization never relies on `Arc` identity; metadata-only updates are visible | Slice 2 equal-content and metadata-only tests |
| Contract parity | Slice 3a and 3b scenarios run in process and over IPC, comparing results, digests, statuses, and rows |
| One host-effect recipient; no duplicate or replay | Slice 4 tests across embedded and IPC viewers, disconnect, reconnect, and timeout |
| Version and capability negotiation | Slice 1 negotiation tests; vectors for every minor version; a decoder test that skips appended fields and defaults missing ones |
| Incompatible versions, malformed and oversized messages, stale incarnations | Slice 1 decoder and negotiation tests, slice 3a incarnation test, fuzz targets |
| Partial writes and slow or disconnected consumers | Slice 5 byte-split tests; slice 3b stall and disconnect tests |
| Retry behavior | Slice 3a in-flight query and mutation tests; slice 3b refused-input resend test |
| PTY parsing and other viewers continue when a client stalls | Slice 3b stalled-client test that asserts another viewer's updates and PTY output progress |
| `wasm32-unknown-unknown` | CI check of `huterm-protocol`, `huterm-wire`, and the sans-IO `huterm-client` with their test targets; the vectors compile into that build and decode in native tests |
| Architecture boundaries | New `mise run architecture` rules, each shown to fail once against a deliberate violation |
| Peer authentication and socket permissions | Slice 5 tests, including a second-user test in Docker |

Property tests use `proptest` as a dev-dependency, synchronize on observable
progress, and bound their case counts so the normal suite stays fast. The
nightly fuzz job is evidence, not a gate; a crash becomes a regression test
in the normal suite. Native desktop behavior does not change in this plan,
so no smoke changes are needed until [#45][issue-45].

## Risks

- **Hand-written codec drift.** Exhaustive destructuring, golden vectors per
  minor version, round-trip property tests, and fuzzing cover it. The cost
  is more code than a derive.
- **Minor version discipline.** Additive evolution only works if each change
  is classified correctly. The vector test catches byte changes; review must
  catch semantic ones. Keep the rule in the dependencies guide or a new
  `huterm-wire` section of the core runtime guide.
- **Update cost under continuous output.** Each credited update builds a
  snapshot and walks its rows. Credit and frame pacing bound the rate; slice
  3b adds a benchmark of output throughput with one remote viewer against
  the in-process baseline.
- **Thread count.** Three threads per connection at 64 connections is about
  200 threads. Acceptable for a local server; an event loop can replace the
  reader and writer threads later without changing the contract.
- **Disconnect versus revocation.** The core change in slice 3b touches the
  viewer lifecycle that the viewers plan settled. Its test must show that
  detach after drop keeps admitted input, and that explicit detach of a live
  viewer still revokes.
- **`/tmp` placement.** Linux systems that clean `/tmp` by age could remove
  a socket under a long-running server. `XDG_RUNTIME_DIR` avoids that where
  it exists; [#45][issue-45] owns stale-socket detection and recreation.
- **The nightly toolchain.** A second toolchain adds cache and bump work.
  The fuzz job is scheduled and optional, so a broken nightly never blocks a
  pull request.

## Corrections to #44's text

Proposed edits to the issue, for the maintainer to apply:

- "Serialize the envelope's `StreamId`": the wire sends the incarnation once
  per connection in `Welcome` and only the sequence per event; the client
  rebuilds the `StreamId`.
- "Stable IDs scoped to server incarnation": add that `TerminalId` becomes a
  scoped ID and that `RuntimeId` allocation moves into `huterm-protocol`.
- "Update the protocol dependency policy deliberately": the policy does not
  change; serialization lives in `huterm-wire`, and the gate gains rules.
- The tests criterion gains property tests and nightly cargo-fuzz targets
  for the codec and update application.
- Dependencies: #22 has landed.

## Open questions

None blocking. The starting values in the bounds table are proposals that
each slice may tune with evidence, recorded in this plan when they change.

[issue-18]: https://github.com/jimeh/huterm/issues/18
[issue-21]: https://github.com/jimeh/huterm/issues/21
[issue-22]: https://github.com/jimeh/huterm/issues/22
[issue-23]: https://github.com/jimeh/huterm/issues/23
[issue-26]: https://github.com/jimeh/huterm/issues/26
[issue-42]: https://github.com/jimeh/huterm/issues/42
[issue-44]: https://github.com/jimeh/huterm/issues/44
[issue-45]: https://github.com/jimeh/huterm/issues/45
[issue-46]: https://github.com/jimeh/huterm/issues/46
[issue-48]: https://github.com/jimeh/huterm/issues/48
[issue-80]: https://github.com/jimeh/huterm/issues/80
[issue-88]: https://github.com/jimeh/huterm/issues/88
[issue-98]: https://github.com/jimeh/huterm/issues/98
[issue-131]: https://github.com/jimeh/huterm/issues/131
[issue-132]: https://github.com/jimeh/huterm/issues/132
[issue-133]: https://github.com/jimeh/huterm/issues/133
[issue-134]: https://github.com/jimeh/huterm/issues/134
