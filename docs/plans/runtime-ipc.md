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
- **At-most-once retry.** A reconnect never repeats a request that may have
  committed or input the server admitted. In-flight requests whose outcome
  is unknown report it, and the resynchronized state shows what happened.
  Only runtime queries are retried. The maintainer confirmed on 2026-11-04
  that shared setters count as mutations, narrowing the set first
  presented, which also retried set-to-value requests.
- **Name cap.** Custom names are limited to 256 bytes in every mode, so
  every hierarchy snapshot stays encodable and bounded (2026-11-04).
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
  `&'static str`, so a decoded identifier must come from a catalog lookup;
  `CellText` has no length limit; `ClipboardWrite`,
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
- `huterm-protocol`, `huterm-wire` with its golden vectors, and
  `huterm-client` without its native transport build for
  `wasm32-unknown-unknown`. The vectors live in a `vectors` module behind a
  feature, so that build compiles them; test targets stay native, because
  `proptest`'s default features need a random source that target lacks.
  `rust-toolchain.toml` gains the target, and
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

After the preamble, the stream is a sequence of frames. A frame is a
little-endian `u32` length, then a flags byte (final, abort), a varint
message stream ID, and message bytes. The length counts everything after
itself, header included, and never exceeds the frame limit. A zero
length, a length above the frame limit, or an unknown flag is a protocol
violation that closes the connection, because a byte stream cannot
resynchronize after a bad header. The frame limit is `HANDSHAKE_FRAME_LIMIT`
(16 KiB) until the handshake completes, then the negotiated `max_frame`: the
smaller of both peers' values, 1 MiB by default and never below 64 KiB.

**Every message may span frames.** A message is the concatenation of the
bytes its stream's frames carry, ending with the frame that sets the final
flag. Most messages fit one frame; any message may take several, split at
any byte, so frame size never limits what can be encoded: a large paste, a
64 MiB clipboard write, a hierarchy snapshot, and a terminal row larger than
a frame all use the same mechanism, so there is one framing rule rather
than one per payload kind:

- **Streams.** The sender picks a stream ID for each message from IDs it
  has no message in flight on; the first frame on a free ID starts a new
  message. Frames of different streams interleave, so a
  large snapshot cannot hold up a keystroke echo in another terminal. Each
  direction may have at most `MAX_OPEN_STREAMS` (4) unfinished messages; a
  sender waits rather than open a fifth, and receiving a fifth is a protocol
  violation. One viewer's updates are serialized (see terminal updates).
- **Caps, not trusted lengths.** The first bytes of a message are its kind,
  and every kind has a byte cap (`MessageCaps` in `huterm-wire`). A message's
  record still starts with its length, but the receiver checks that length
  against the kind's cap before buffering, checks the cap again as each frame
  arrives, and checks every length prefix inside the message against its field
  cap before allocating, so it never reserves memory on a sender's word. A
  message over its cap is a protocol violation. Decoding may be incremental or
  run once the final frame arrives; the caps bound memory either way.
- **Where caps come from.** Payload limits and message caps are separate.
  A payload limit bounds what a user or terminal supplies: paste bytes,
  selection text, clipboard text, effect responses. Core invariants bound
  server state: grid cells, cell text, names, hierarchy items, status text.
  A message cap is computed by one function in `huterm-wire` from those
  limits plus the worst-case encoding overhead of its kind: varint length
  prefixes, record headers, run headers, style-table entries, and effect or
  request metadata. A payload at its limit, or state at every invariant's
  limit, therefore always fits its message. A sender checks payload limits
  first and refuses an oversized payload locally with `LimitExceeded`, as
  the desktop refuses an oversized paste today. For each kind, slice tests
  encode the worst case the limits allow and check it fits the computed
  cap.
- **Caps follow the negotiated minor version.** `MessageCaps` is a
  function of the negotiated minor version, and `Welcome` advertises the
  caps that bind the client. A minor version that appends fields to a kind
  a peer on an older minor version receives must fit those fields within
  the older version's cap, or project them away for that peer, so a newer
  sender never pushes an older receiver over its cap.
- **Server memory.** The server holds at most `SERVER_REASSEMBLY_BUDGET`
  of unfinished client messages: `MAX_OPEN_STREAMS` times the largest client
  message cap, which is a paste at its limit, so a little over 4 MiB.
  `Welcome` advertises it with every cap that binds the client. A client
  waits rather than exceed it, so a well-behaved client never meets the
  limit, and exceeding it is a protocol violation.
- **Abort and idle.** A sender abandons an unfinished message with a frame
  that sets the abort flag, after the frame already being written, and the
  receiver frees the stream. The sender queues a large message's frames one at
  a time, each a copy of at most `max_frame` bytes, so abandoning a message
  releases its source, such as a host effect's reservation, immediately; only
  the frame already in the writer remains, and the writer-stall rule bounds
  how long that frame can take. A stream that receives no frame for
  `STREAM_IDLE_DEADLINE` (30 s) is a protocol violation; the server pauses
  that clock while it has itself stopped reading the connection.

A message is a message kind, as a varint, followed by that message's record.
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

### Versions, features, and server incarnation

The client sends `Hello` first. Its first two fields, the major and minor
versions, keep their position and encoding in every major version, so any
server can read them and answer `IncompatibleMajor`. `Hello` carries:

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
versions, `ServerBusy`, or `Malformed`) and a diagnostic message. The
message kinds of `Hello` and `Reject`, and `Reject`'s reason and version
fields, keep their position and encoding in every major version, as
`Hello`'s leading fields do, so a client of another major version reads a
clear version error. Any message before `Hello` other than `Hello` itself,
or no `Hello` within `HANDSHAKE_DEADLINE` (5 s), closes the connection.

A minor version may only add messages, enum variants, and appended fields
that a peer on an older minor version can do without. Senders withhold
messages and variants newer than the negotiated minor version. They always
write appended fields, and an older decoder skips them; the field must
therefore be safe to ignore. A change an older peer cannot do without is a
major version change.

Every addition that a server can send defines its downgrade for older
minor versions, and withholding must never break a sequence:

- An older client receives at most one event envelope for each server
  sequence number, and a resync snapshot covers any it does not receive, so
  `committed_seq` values stay shared and the reducer sees no gap.
  A newer event variant reaches it as exactly one of these:
  - `HierarchyEvent::Skipped`, reserved in minor version 0, which advances
    the sequence and changes nothing, when the variant changes no state the
    peer's minor version holds. It must exist in minor version 0, because an
    older client can never learn it later.
  - The single older event that produces the same older-profile state, when
    one exists.
  - Otherwise, a `HierarchyResync` carrying a snapshot projected onto the
    peer's minor version at that sequence, which the client installs through
    the existing resync path. This costs a snapshot per such event, so a
    variant that would take this path often should define a one-event
    downgrade instead.
- A newer cell style, status field, or snapshot field is projected onto the
  peer's minor version, such as an extended underline style shown as a plain
  underline for [#83][issue-83].
- Digests cover exactly the fields of the negotiated minor version (see the
  hierarchy and terminal update sections), so a projected state and its
  digest always agree.

A test-only minor version in `huterm-wire` exercises these rules before a
real minor version 1 exists: it appends a hierarchy field, adds three
event variants, one for each downgrade path above, and adds a cell style
flag. A server at that version must converge with a minor-0 client, sending
the variants as `Skipped`, as their one older event, and as a projected
`HierarchyResync`, with no digest mismatch and no client-initiated
resync.

Features gate optional behavior inside a minor version: rich content
([#88][issue-88]) is the first reserved feature. Attachment events
([#26][issue-26]) are expected to arrive as a minor version, which older
clients receive as `Skipped`. A client that lacks a feature still receives
complete text-terminal operation.

Golden vectors pin the encoding: `huterm-wire`'s `vectors` module holds
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
outstanding per connection, and at most one of them may be a request whose
result can exceed a frame, such as selection text. Past either limit the
server answers `Busy` without acting. `Cancel { request }` asks the server to
abandon a request that has not committed. The request still gets exactly
one result.

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
`DesktopRuntime` and `TerminalViewer` today, plus catalog commands. Slice 3a
adds the inventory. Two kinds of state stay on the server instead of
crossing the wire as values:

- **Close tickets.** `prepare_close` returns a ticket ID bound to the
  connection, at most 16 per connection. Check and commit name the ticket.
  The existing revision check still turns a changed hierarchy into
  `StaleClose`, and a disconnect discards the connection's tickets.
- **Layout history.** A rearrangement request names the history slot it
  belongs to, a client-chosen `u32` that the desktop uses per window. Its
  result returns an opaque undo token, and the server keeps the
  `LayoutChange` it names. Each slot holds the last change each way, as
  layout history does today, so a new change replaces the slot's earlier
  token. A connection may use at most `MAX_HISTORY_SLOTS` (64) slots; past
  that, the least recently used slot is evicted, and its tokens answer
  `StaleUndo`. A client cannot forge a record, and a disconnect discards the
  tokens, so undo is unavailable after reconnect.

A request already running on a worker when its connection ends runs to
completion. Whatever it created for the connection, such as a viewer, a
close ticket, or an undo token, is released when it completes, and its
structural events still reach every live subscription.

### Retry and reconnect

Every request kind has a static retry class:

| Class | Examples | After a broken connection |
| --- | --- | --- |
| Runtime query | List or select targets, read hierarchy | Retried on the next connection |
| Viewer request | Snapshot, scroll, selection text, link lookup, report geometry, set focus, submit presentation | Not retried; fails with `Disconnected`. The old viewer is gone; the client subscribes a new one, sends its current state, and the new viewer's first update replaces any lost snapshot |
| Mutation | Spawn, split, close, move, arrange, reorder, rename, set input sync, resize split, set zoom, set scrollback, clear history, reset | Never retried; resolves `OutcomeUnknown` |

Setters of shared runtime state count as mutations. Retrying one after an
unknown outcome could overwrite another client's later change, and
`set_scrollback` evicts history when it lowers a limit, so a retry could
destroy rows that a later change kept. This narrows the retry set presented
during the decision, which also listed set-to-value requests; the
maintainer confirmed the narrowing.

Reconnecting resumes from state, not from a log. Hubris resumes a terminal
from a PTY byte offset, replaying missed output from a ring buffer and
falling back to scrollback plus a screen-state snapshot, because its browser
client runs the emulator and must see every byte. Huterm's emulator and
scrollback stay on the server, so the fallback path is the only one it
needs: the client gets the current state, which is one viewport and its
modes, cursor, and viewport metadata, and fetches history only when the user
scrolls into it. Each stream's number still tells the client what changed:
the hierarchy sequence, the terminal's publication revision, and the status
baseline above. Two Hubris-style resume paths were considered and rejected:

- A bounded hierarchy event ring keyed by sequence, so `SubscribeHierarchy`
  could replay from the client's last sequence. It would save a snapshot of
  a few KB per reconnect at the cost of a second recovery path, and
  reconcile already derives views from state.
- Server-retained viewer baselines for a grace period, so a reconnect could
  receive a delta. It would save one viewport per reconnect, which is not
  worth retaining baselines locally. [#49][issue-49]'s remote attachment
  over slow links may revisit it.

When a connection breaks, every in-flight mutation resolves `OutcomeUnknown`
and every viewer request fails with `Disconnected`, at once, whether or not
the client reconnects. Runtime queries wait for the next connection or fail
with `Disconnected` when the client gives up. After reconnecting, the client
resubscribes to the hierarchy and installs the new snapshot. A mutation's
effect, if it committed, is visible there or in a later event, and the
level-triggered reconcile installs or removes views from state. A tab whose
spawn result was lost therefore appears as a tab the window did not spawn,
which reconcile already handles.

Input is not request-and-result. Input admitted by the server is never
resent, so a keystroke typed during a disconnect can be lost but never
duplicated. Input the server refused before admission is resent in order
on the same connection (see input below). Host effects are never resent or
replayed.

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
that sequence sends `ResyncHierarchy`, unless the mismatch is on a snapshot
itself. Then, as for terminal updates, another snapshot would mismatch the
same way, so the client reports a contract fault and disconnects instead of
looping.

- **Algorithm.** `HierarchyState::digest()` keeps its documented canonical
  encoding, which is independent of the wire codec; a golden vector pins
  it. A minor version that adds digested state adds it only to the digest
  for peers at that version or later. `huterm-protocol` therefore gains a
  `DigestProfile` type, which names the set of digested fields without
  knowing about wire versions; `huterm-wire` maps each negotiated minor
  version onto a profile.
- **Where it is computed.** Core computes the digest at emission, under the
  Mux lock, from a state it builds from Mux's own structure, for each
  profile its subscribers registered. A connection's forwarder does not run
  a reducer of its own, so a reducer bug cannot produce matching digests on
  both ends. Building that state costs time proportional to the hierarchy,
  under the Mux lock, for each registered profile, on every structural
  result, every subscription snapshot, and every 256th event. Hierarchies
  hold tens to hundreds of items, so this stays small, and slice 3a measures
  it on a large fixture.

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

A terminal whose runtime stopped on its own, by a panic or a fatal I/O
failure, stays in Mux, but core refuses to register a viewer on its closed
registry (`Registry::register`, `TerminalViewer::subscribe`). Its final
status stays readable after close. `SubscribeTerminal` on such a terminal
therefore answers `Stopped` together with that final status, read under the
same attachment and session check as a subscription, so a client still
learns the terminal's exit and failures, including after a reconnect. Core
gains that authorized read of a closed registry's status.

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

**Each delta names its baseline.** A viewer's updates carry an
`UpdateSeq` that starts at 1 and advances by one per update, and a delta
names the `UpdateSeq` it builds on. A client applies a delta only to that
baseline. On a mismatch it discards the delta, sends one `ResyncViewer`, and
discards every further delta until the complete snapshot that answers it
arrives. `ResyncViewer` wakes the viewer's pump even when the viewer has no
pending notification. The viewer's pump is the only producer of its updates
and finishes one update's message before it starts the next, so one
viewer's updates never interleave with each other, although they interleave
with other traffic. A missing, reordered, or misapplied delta is therefore
detected at once, not at the next digest.

**Complete snapshots.** An update has no baseline, and every row is in full,
when the viewer is new or reconnected, when the geometry revision changed
since the baseline, or when the client sent `ResyncViewer`. Recovery is
therefore always one complete snapshot.

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
`huterm-protocol` gains `MAX_CELL_TEXT_BYTES` (1 KiB), and core truncates a
longer grapheme cluster at a character boundary when it extracts a row, on
every path. The encoder therefore never meets a cell it cannot send, and
in-process and IPC rows stay identical. The decoder rejects longer cell
text, and checks that every row has exactly `size.columns` cells, that there
are exactly `size.rows` rows, that every `Keep` index exists in its
baseline, and that every style index exists. An update that fails any
check is a protocol violation.

**Digests.** Every complete snapshot, and every 64th delta
(`UPDATE_DIGEST_INTERVAL`), carries a 64-bit FNV-1a digest of the canonical
encoding of the resulting rows and metadata. A client whose result differs
after a delta sends `ResyncViewer`. A mismatch on a complete snapshot
cannot be fixed by another one, because the same state would mismatch
again, so the client reports it as a contract fault, closes that viewer,
and sends no resync. The digest is computed only at those points, so steady
state pays nothing for it. Like the hierarchy digest, it covers exactly the
fields of the negotiated minor version.

**Credit paces updates.** The client grants each viewer credit, at most
`MAX_VIEWER_CREDIT` (2) outstanding updates. Each update says whether it
spent a credit. The client returns a credit when it has disposed of a
credited update, whether it applied it or discarded it as stale, so a
discarded delta never leaks credit. The first update of a subscription, the
update that answers a scroll, and the complete snapshot that answers
`ResyncViewer` spend none, so recovery never waits for credit. The server's
pump for a viewer waits on the viewer's wake and on credit grants; when the
viewer is notified and has credit, it requests a snapshot, encodes the
update, and spends a credit. A credit grant that raises a notified viewer's
credit from zero wakes the pump itself, so the update never waits for
another publication. A client grants no credit to a hidden tab, so its viewer stays
notified, the runtime does not wake it again, and a hidden terminal costs
the wire no row data until it is shown. This is the in-process rearm rule
carried across the wire.

**Status is state.** When a viewer's status revision changes, the server
puts the whole `TerminalStatus` in the viewer's single pending status slot,
replacing any unsent one, and sends it at once, ahead of any update not yet
written. Status does not spend credit and never waits for an update, so a
hidden tab still reports its exit, bells, and title to tab indicators. The
client derives exits, new bells, and missed failures from its own
baseline. The derivation in `TerminalViewer::poll` moves into a
`StatusBaseline` type in `huterm-protocol` so both clients share one
implementation.

**Reconnect keeps the status baseline.** A new viewer normally takes the
status it first reads as its baseline, so it reports no exit that happened
before it. A client that reconnects to the same server incarnation and
subscribes again to a terminal it already showed keeps that terminal's
previous `StatusBaseline` instead. Status revisions, the bell count, and
failure numbers are terminal-wide and never go backwards within an
incarnation, so the first status after reconnecting reports an exit, bells,
and failures that happened while the client was away, and drives
`close_on_exit` and bell handling as if it had never left. A new incarnation
starts fresh baselines, because its terminals are new.

**Scrolling.** A scroll request is answered through the viewer's pump: the
pump applies `request_scrolled_snapshot`, sends the resulting update without
spending credit, then sends the request's result. If an update message is
in progress, the scroll's update follows it. Scroll numbers must be the
client's: today the server-held `TerminalViewer` would number scrolls itself
(`fetch_add` in `request_scrolled_snapshot`), and a scroll the server
refused would make the client's `InputStamp.scrolls` run ahead of the
viewer's count, so typing could undo a later scroll. Core gains a scroll
request that takes the number the client assigned, and the server passes
the client's stamps through unchanged. The return-to-live rule then needs no
terminal-wide counter, as the viewers plan intended.

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
- A paste larger than one frame, up to exactly `PASTE_PAYLOAD_LIMIT`, is a
  multi-frame message that the server reassembles into one
  `TerminalInput::Paste` before admission, so bracketed paste stays atomic.
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
   disconnect reaches the PTY. The change also applies in embedded mode: a
   closing window's admitted input now reaches the PTY instead of being
   discarded, which matches the viewers plan's rule for dropped viewers.
   Explicit detach of a live viewer still revokes it.
3. The server discards the connection's close tickets, undo tokens, staged
   input, unfinished messages, and host-effect recipients, and resolves its
   pending host-effect responses with their fallbacks.

A disconnect never closes a terminal, workspace, or session.

### Bounds and backpressure

| Bound | Starting value | Exceeded |
| --- | --- | --- |
| `HANDSHAKE_FRAME_LIMIT` | 16 KiB | Close |
| `max_frame` | 1 MiB, at least 64 KiB | Close |
| `HANDSHAKE_DEADLINE` | 5 s | Close |
| `MAX_CONNECTIONS` per server | 64 | `Reject { ServerBusy }` |
| `MAX_IN_FLIGHT_REQUESTS` per connection | 64, of which 1 may have a multi-frame result | `Busy`, retry safe |
| `MAX_CONNECTION_VIEWERS` | 256 | `LimitExceeded` |
| Viewers per terminal | 32, as today | `LimitExceeded` |
| `MAX_VIEWER_CREDIT` | 2 | Protocol violation |
| `VIEWER_INPUT_BUDGET` | 1 MiB | Staged, then `InputRefused` |
| `HIERARCHY_LANE_BUDGET` per connection | 1 MiB | Lag, then `HierarchyResync` |
| `OUTBOUND_BUDGET` per connection | 8 MiB | Producers wait; inbound reading pauses |
| `WRITER_STALL_DEADLINE` | 30 s | Disconnect |
| `MAX_OPEN_STREAMS` per direction per connection | 4 | Sender waits; receiving more is a protocol violation |
| `SERVER_REASSEMBLY_BUDGET` per connection, advertised in `Welcome` | `MAX_OPEN_STREAMS` times the paste message cap, about 4 MiB | Sender waits; receiving more is a protocol violation |
| `STREAM_IDLE_DEADLINE` | 30 s, paused while the server is not reading | Protocol violation |
| `CLIPBOARD_PAYLOAD_LIMIT`, `SELECTION_PAYLOAD_LIMIT` | 64 MiB of text each | Sender refuses with `LimitExceeded` |
| `PASTE_PAYLOAD_LIMIT` | `VIEWER_INPUT_BUDGET` | Sender refuses with `LimitExceeded` |
| `EFFECT_RESPONSE_PAYLOAD_LIMIT` | 64 KiB | Sender refuses with `LimitExceeded` |
| `MessageCaps`, per kind and negotiated minor version | The kind's payload limit or core invariants plus its worst-case encoding overhead; 64 KiB for requests without a payload | Receiving more is a protocol violation |
| `MAX_GRID_CELLS`, a core invariant | 1,048,576 cells | Core clamps every grid it is given: spawn, split, resize, and geometry reports |
| `MAX_CELL_TEXT_BYTES`, a core invariant | 1 KiB | Core truncates at row extraction |
| Update message cap | From the grid an update's header declares: per cell, the cell-text limit plus its length prefix, run header, and style-table entry; per row and per update, their headers | Protocol violation |
| `MAX_NAME_BYTES`, a core invariant | 256 bytes | Custom names: `InvalidName`; fallback names: truncated |
| `MAX_HIERARCHY_ITEMS`, a core invariant | 16,384 sessions, workspaces, tabs, panes, and splits together | `LimitExceeded` from the structural operation |
| Hierarchy message cap | Computed from the two invariants above and the encoding overhead per item | Protocol violation |
| `MAX_STATUS_TEXT_BYTES`, a core invariant | 4 KiB per title, metadata string, and failure message | Core truncates when it publishes status |
| Status message cap | Computed from `MAX_STATUS_TEXT_BYTES` for each text field and the failure log's 8 entries | Protocol violation |
| `MAX_CLOSE_TICKETS` per connection | 16 | `LimitExceeded` |
| `MAX_HISTORY_SLOTS` per connection | 64 | Least recently used slot evicted; `StaleUndo` |
| `HOST_EFFECT_DELIVERY_DEADLINE` | 2 s from the head of the lane to the first frame | Effect dropped |
| `HOST_EFFECT_MIN_THROUGHPUT` | Completion within the deadline plus 1 s per 8 MiB | Message aborted, effect dropped |
| `CONNECTION_EFFECT_SHARE` per connection per terminal | All 8 with one connection; 8 divided by the connections with recipients, at least 2, otherwise | Later effects for that connection's recipients denied |
| `MAX_PENDING_EFFECT_RESPONSES` per connection | 8 | Fallback answer, as for a missing client |
| `HIERARCHY_DIGEST_INTERVAL`, `UPDATE_DIGEST_INTERVAL` | 256 events, 64 deltas | Not a limit |

Every outbound source is bounded before it reaches the queue: results by
in-flight requests and the single multi-frame result, updates by credit,
status by one slot per viewer, hierarchy events by the lane budget and lag,
and host effects by their lane. Producers also check `OUTBOUND_BUDGET`:
while the queue is over it, pumps and large results wait, and the server
stops reading that connection's inbound frames, so a client that floods
requests without reading meets socket backpressure instead of a growing
pile of `Busy` results.

Encoding memory is bounded too. A producer takes a stream slot and
reserves the message's first frame against `OUTBOUND_BUDGET` before it
encodes anything, and then encodes one frame at a time, each reserved the
same way. A deterministic sizing pass computes the record length that opens
the message without building it. A pump keeps its snapshot, an `Arc` core
already holds, rather than an encoded copy, so a connection's encoded
output never exceeds `OUTBOUND_BUDGET` plus one frame per open stream,
however many viewers it has.

The server disconnects a client when either condition lasts for
`WRITER_STALL_DEADLINE`: bytes are pending and the socket accepts none, or
the queue stays over `OUTBOUND_BUDGET` however slowly the client reads. The
deadline is armed only while one of the conditions holds, so an idle or
keeping-up connection arms nothing. Tests drive every deadline in
`huterm-server` and `huterm-client` through an injectable clock, so none of
them depends on elapsed wall-clock time.

The values are starting points, named in `huterm-wire` or `huterm-server`,
and each has a boundary test. Exceeding `MAX_IN_FLIGHT_REQUESTS` is answered
with `Busy`; exceeding any other negotiated limit, such as credit, frame
size, open streams, or a message cap, is a protocol violation that
disconnects the client. A server never sends more than it advertised.

The core invariants keep every state the server holds encodable. Each one
is enforced on every path that produces the value, in embedded mode as
well, so in-process and IPC clients see the same state:

- Grids larger than `MAX_GRID_CELLS` are clamped, keeping their aspect, on
  spawn, split, resize, and every geometry report. Today `split_pane` and
  spawn pass `TerminalCommand.grid_size` straight to the runtime, and
  `GridSize::clamped` enforces only a minimum.
- Custom names longer than `MAX_NAME_BYTES` are rejected with
  `InvalidName`, as blank names are today; launched-program fallback names
  are truncated at a character boundary. Today `validate_name` checks only
  for blank names.
- A structural operation that would exceed `MAX_HIERARCHY_ITEMS` fails with
  `LimitExceeded` before it changes anything.
- Titles, metadata strings, and failure messages longer than
  `MAX_STATUS_TEXT_BYTES` are truncated at a character boundary when status
  is published.

Each invariant changes embedded behavior only for pathological input, and
each gets a core test.

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

Selection keeps today's rule, one eligible recipient by activity ordinal,
chosen at admission and never redirected, with one addition: a recipient is
eligible for an effect only if its client negotiated that effect kind. A
later minor version that adds a kind therefore never selects an older client
that would have to drop it while a capable client is attached.

**Wakes reach the effect lane.** Admission signals the recipient through
its viewer's own wake (`RecipientLink.activity` in `host_effects.rs`), the
same wake that publications use. Every time a viewer's task wakes, it checks
status, offers the recipient's queued effects to the connection's
host-effect lane, and only then considers an update, which needs credit. A
hidden viewer with no credit therefore still delivers its effects, as the
embedded desktop does when it drains effects on every refresh. Offering
effects never waits for an update: the pump encodes one frame at a time, so
it reaches the lane between frames of a long update. The lane holds one
effect at a time; when it frees, it takes the next queued effect from the
connection's recipients with pending effects, so an effect queued behind a
large write is delivered without another wake.

**Effect frames go first.** The writer sends the lane's next frame ahead of
queued update and result frames, and the lane reserves its own one-frame
slot rather than waiting on `OUTBOUND_BUDGET`. A busy connection therefore
delays an effect's first frame by at most the frame already in the
writer.

The lane pops a delivery, rechecks `is_current`, assigns a
connection-scoped `EffectId`, and sends `HostEffect { effect, viewer,
permission_generation, terminal, kind, payload, response }`. Large
payloads span frames like any message.

- **Permission travels with the effect.** In process, clipboard permission
  lives on the recipient, revoking it cancels queued and popped deliveries,
  and the desktop rechecks `is_current` immediately before the OS call
  (`host_effects.rs`, `HostEffectRecipient::set_allowed` and `is_current`).
  Over IPC the client owns the permission and numbers its changes per
  viewer. `SetHostEffectPermission { viewer, generation, allowed }` is an
  ordered message, not a request, so it is never refused with `Busy`; the
  server applies it to the recipient and stamps each effect it admits with
  the generation it last received. The client executes an effect only if
  that viewer is still live and the effect's generation equals its current
  one. An effect already sent when the user revokes permission is discarded
  on the client, and a revoke followed by a re-grant cannot revive it.
- **No replay.** Effect IDs belong to one connection. A disconnect revokes
  the connection's recipients, which clears their queues, and nothing is
  ever re-sent on a later connection. A client ignores effect frames it
  cannot match to its current connection.
- **Delivery deadlines.** Core records each effect's admission time, which
  `PendingHostEffect` does not hold today. An effect waits in its
  recipient's queue only behind earlier effects in the lane, each bounded by
  its own deadlines below. Once an effect reaches the head of the lane, its
  first frame must be written within `HOST_EFFECT_DELIVERY_DEADLINE` (2 s),
  and its last frame within that deadline plus one second per
  `HOST_EFFECT_MIN_THROUGHPUT` (8 MiB) of payload, 10 s for a 64 MiB write.
  An effect that misses either deadline is dropped at once, releasing its
  reservations, and its message is aborted after the frame already in the
  writer. A healthy client therefore receives every effect, including one
  queued behind a maximum-size write, and a stalled remote recipient holds
  the terminal's effect and byte budgets for a bounded time, rather than
  denying every other recipient indefinitely, which the viewers plan
  flagged.
- **Budgets.** Each connection gets its own process-level budget, as each
  `DesktopHostEffectClient` does today (32 effects and 128 MiB). A terminal
  queues at most 8 effects, as today. While recipients from only one
  connection are registered on a terminal, that connection may use all 8,
  so a lone client keeps today's queue depth for bursts such as a clear
  followed by several sets. While recipients from several connections are
  registered, each connection's recipients together may hold at most
  `CONNECTION_EFFECT_SHARE` of them: 8 divided by the number of those
  connections, and at least 2. A connection over its share when another
  registers keeps its queued effects, which drain within their deadlines,
  and its new effects are denied until it is under its share. Bytes stay
  shared up to the terminal's 64 MiB, the Kitty protocol's minimum write
  size, so a recipient that stops reading in the middle of a maximum-size
  write can deny other recipients' writes to that terminal until its
  completion deadline, at most 10 s after it reached the head of the lane.
- **Response correlation.** An effect that needs an answer carries a
  response deadline. The server keeps a bounded pending entry per effect and
  answers the terminal with the effect's documented fallback on timeout,
  disconnect, or a refusal. A client that cannot send its answer within
  `EFFECT_RESPONSE_PAYLOAD_LIMIT` sends an explicit refusal. A late or unknown
  `HostEffectResult` is ignored.
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
`SO_PEERCRED` on Linux and `getpeereid` on macOS, both through the pinned
`nix`, whose workspace entry gains the `socket` and `user` features in slice
5. It closes any connection whose user ID differs from its own
effective user ID, before the preamble. The client checks the directory the
same way before connecting, so it never talks to a socket another user
planted.

The server serves each connection with a reader thread, a writer thread,
and one executor thread for its pumps and request handling. Mux-locking and
blocking work (spawn, close, assessment) runs on worker threads, never on a
connection's I/O or pump threads. Slice 3a picks the executor from crates
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

Six pull requests for #44, each mergeable on its own. Slices 1 and 2 add
contract code that nothing calls until slice 3a, plus changes to existing
paths: slice 1 migrates core and the desktop to the new identity types, and
slice 2 moves status derivation into `huterm-protocol` and caps cell text at
row extraction. Slice 3b changes what an embedded window close does with
admitted input.

### Slice 1: identity and the wire foundation

- `huterm-protocol`: `ServerIncarnation`, `RuntimeId::allocate`, a scoped
  `TerminalId`, `StreamId` with an incarnation, `HierarchyEvent::Skipped`,
  and `DigestProfile`. Core and the desktop migrate mechanically.
- Core invariants for the hierarchy: `MAX_NAME_BYTES` in `validate_name`
  and for fallback names, and `MAX_HIERARCHY_ITEMS` on every structural
  operation, each with a core test.
- New `huterm-wire`: preamble, frames with streams, abort, and idle rules,
  `MessageCaps`, codec primitives, `Hello`, `Welcome`, and `Reject` with the
  negotiation function, `WireError` and its codes, ID encoding,
  and the encodings for hierarchy state, events, pane layouts, catalog
  invocations, and outcomes.
- The test-only minor version and its downgrade rules for hierarchy state
  and events.
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
version and the feature intersection and rejects a different major version,
reading only `Hello`'s frozen leading fields to do so; a minor-0 decoder
reads a record from the test-only minor version, skipping its appended field,
and a test-only-minor decoder reads a minor-0 record, filling the appended
field's default, while rejecting a record cut short before a minor-0 field;
a paste exactly at `PASTE_PAYLOAD_LIMIT` and a clipboard message exactly at
`CLIPBOARD_PAYLOAD_LIMIT` fit their computed caps, and one byte more is
refused locally by the sender; caps for the test-only minor version fit
within minor 0's for every kind a minor-0 peer receives;
a fifth open stream, a message over its kind's cap, a client message past
the advertised reassembly budget, and an idle stream are rejected by the
receiver, while a conforming sender waits or refuses locally with
`LimitExceeded`; an abort frees the receiver's stream; any message split at
every byte offset decodes identically; a hierarchy at `MAX_HIERARCHY_ITEMS`
with every name at `MAX_NAME_BYTES` encodes under its computed cap
and round-trips at 64 KiB frames; a client of another major version reads
`Reject { IncompatibleMajor }`; an ID from another `RuntimeId` refuses to
encode; the wasm check passes; the gate fails when a dependency is added to
`huterm-wire`.

### Slice 2: terminal updates and status

- `huterm-wire`: the update encoder (baseline, current snapshot, style
  table, `Keep` operations) and the decoder that
  applies an update to a client baseline, with `UpdateSeq` baselines,
  credited flags, status and input messages, and digests with their minor
  version profile.
- `huterm-protocol`: `StatusBaseline`, with `TerminalViewer::poll` moved
  onto it, and the core invariants `MAX_CELL_TEXT_BYTES` at row extraction,
  `MAX_GRID_CELLS` on spawn, split, resize, and geometry reports, and
  `MAX_STATUS_TEXT_BYTES` at status publication, each with a core test.
- A fuzz target that applies arbitrary update sequences to a baseline.

Acceptance:

- For any publication sequence and any subset of it the server chooses to
  encode, a client that applies every sent update in order ends with exactly
  the latest encoded snapshot's rows and metadata.
- A dropped, duplicated, or reordered delta is detected through its
  `UpdateSeq` baseline, never applied, and a resync restores the latest
  state.
- A geometry change yields a complete snapshot.
- A metadata-only change yields an update with no full rows that changes the
  client's cursor, modes, or viewport.
- Two snapshots with equal content but distinct row `Arc`s produce the same
  client state.
- A `Keep` index outside the baseline, a style index outside the style
  table, a run whose cell counts do not fill its row, and an update whose
  header declares a grid above `MAX_GRID_CELLS` or whose bytes exceed the
  cap derived from its grid are each rejected.
- A worst-case update, with every cell at `MAX_CELL_TEXT_BYTES` and its own
  style, fits the cap derived from its grid at a mid-size grid and at
  `MAX_GRID_CELLS`, and a status with every text field at
  `MAX_STATUS_TEXT_BYTES` and 8 failures fits its cap.
- A spawn, split, or resize above `MAX_GRID_CELLS` is clamped in core, and
  its first update encodes under its cap.
- A grapheme longer than `MAX_CELL_TEXT_BYTES` reaches in-process and wire
  rows truncated identically.
- A single row larger than the smallest `max_frame` (128 columns of
  near-limit graphemes at 64 KiB frames) encodes, splits across frames, and
  decodes to the same row, and splitting any update at every byte offset
  decodes identically.
- A test-only minor version cell style flag reaches a minor-0 client
  projected, with matching digests and no resync; a test-only-minor client
  decoding a minor-0 server's updates computes digests with the minor-0
  profile and matches.
- A digest mismatch on a complete snapshot closes the viewer as a contract
  fault without a resync, and a mismatch on a delta resyncs once.

### Slice 3a: server and client, requests and hierarchy

- New `huterm-server` connection handler and `huterm-client` session over
  in-memory streams: handshake, request table, results and errors, cancel,
  incarnation mapping, hierarchy subscription with lag, resync, and digest
  checks, digests computed by core at emission, close tickets, undo tokens
  with history slots, outbound budget gating, stall detection, and the
  request inventory.
- The contract-test harness and its first scenarios: create, rename, move,
  reorder, split, arrange, undo, close with tickets, and stale targets.

Acceptance:

- Each scenario gives the same results and hierarchy digests in process and
  over IPC.
- A forced core lag resynchronizes a slow client without losing a committed
  change.
- A server at the test-only minor version emits its three new event
  variants to a minor-0 client as `Skipped`, as one older event, and as a
  projected `HierarchyResync`, and the client applies the whole stream
  without a gap, a digest mismatch, or a resync of its own, and every
  `committed_seq` waiter resolves through each of the three paths.
- A test-only-minor client against a minor-0 server sends no withheld
  request or variant, computes hierarchy digests with the minor-0 profile,
  and keeps read-your-writes on a live stream with no resync.
- A reconnect to a new incarnation makes old IDs foreign and cancels
  sequence waiters.
- A mutation in flight at disconnect, including a rename and a
  `set_scrollback`, resolves `OutcomeUnknown` at once and is never resent;
  a runtime query in flight is retried on the next connection, and a
  viewer request in flight fails with `Disconnected`.
- A request still on a worker at disconnect releases what it created for
  the connection when it completes.
- Arranging in more than `MAX_HISTORY_SLOTS` slots evicts the least recently
  used slot, whose tokens answer `StaleUndo`, and server memory for undo
  stays bounded.
- A client that floods requests without reading meets socket backpressure,
  not a growing queue; a client that reads a byte at a time while over
  `OUTBOUND_BUDGET`, and one that stops reading while under it, are both
  disconnected at `WRITER_STALL_DEADLINE` on the injected clock.

### Slice 3b: viewers, updates, and input over IPC

- Remote viewers, update pumps with credit, status slots, scrolling, input
  sequencing with staging, `InputRefused`, `ResumeInput`, and
  `InputCapacity`, per-viewer input budgets in core, client-numbered scrolls
  in core, and the disconnect order with its core change.
- Contract scenarios for snapshots, scrolling, input, focus, geometry
  control, and revocation.

Acceptance:

- A fast and a slow client on one terminal converge on the same rows and
  metadata while the slow one skips publications.
- A client that stops reading stalls neither PTY parsing nor the other
  viewer, and is disconnected after `WRITER_STALL_DEADLINE`.
- One connection's large paste does not refuse another connection's
  keystrokes.
- Input admitted before a disconnect reaches the PTY, and input after it
  never does; explicit detach of a live viewer still discards its queued
  input.
- A hidden viewer without credit causes no updates during output, and
  still delivers its terminal's exit and bells through status.
- A pump whose credit was spent on deltas the client discarded as stale
  still delivers the complete snapshot that answers `ResyncViewer`, and the
  viewer then resumes normal updates.
- An embedded window close keeps input the window's viewer sent before
  closing, which reaches the PTY; the viewers smoke and the close smokes
  stay green.
- A credit grant to a notified viewer with no credit sends the pending
  update even when no further output arrives.
- A client that reconnects to the same incarnation reports an exit, bells,
  and failures that happened while it was disconnected, including on a
  terminal whose runtime stopped on a fatal failure meanwhile, which answers
  `Stopped` with its final status; one that reconnects to a new incarnation
  starts fresh baselines.
- With 256 viewers on large grids under heavy output, a connection's
  encoded output stays within `OUTBOUND_BUDGET` plus one frame per open
  stream.
- A scroll the server refuses does not let later typing undo a scroll the
  user made after it.
- A scroll during a multi-frame update arrives after that update, and the
  client applies both.

### Slice 4: host effects over IPC

- Recipient origins by client kind, eligibility by negotiated effect kind,
  wake routing to the effect lane, permission messages and generations,
  admission times in core, delivery and completion deadlines,
  per-connection budgets, response correlation, and disconnect
  cancellation.

Acceptance:

- An OSC 52 write reaches exactly one eligible desktop client across
  embedded and IPC viewers, and a text client is never selected.
- An effect pending at disconnect is not delivered on reconnect or to
  another client.
- An effect sent before the client revokes clipboard permission is not
  executed, including after a re-grant that arrives before it is handled,
  and later effects are executed normally after any number of permission
  changes.
- On the injected clock: a 64 MiB clipboard write, exactly at
  `CLIPBOARD_PAYLOAD_LIMIT`, that takes 8 s on a reading client completes;
  a small effect for another terminal on the same connection, queued behind
  it, is delivered after it; an effect's first frame on a connection with
  queued update frames goes out after at most the frame already in the
  writer; and an effect whose client stops reading, including one that
  trickles a byte at a time mid-frame, is dropped at its deadline with its
  reservations released at once, after which another recipient's write to
  the same terminal is admitted and the connection's framing stays valid.
- With one connection attached, a burst of three or more OSC 52 writes in
  one read batch is admitted, and the client ends with the last write; with
  two connections, one at its `CONNECTION_EFFECT_SHARE` does not deny
  effects to the other, whatever number of its viewers are recipients.
- A hidden viewer with no credit receives an OSC 52 write, and an effect
  queued behind a 64 MiB write on the same connection is delivered once
  that write completes, with no further wake.
- Response correlation ignores a result with an unknown or stale
  `EffectId`, ignores a late result after its deadline's fallback, and
  answers the fallback at once on disconnect.
- A recipient that stops reading cannot block later effects for other
  recipients.
- A timed-out response produces the fallback without stalling terminal
  output.

### Slice 5: Unix socket transport and authentication

- Path resolution, directory and socket permission checks, peer
  credentials with the `nix` feature additions, the listener with
  `MAX_CONNECTIONS`, the handshake deadline, and the native client
  transport's `connect`.

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
| Fast and slow clients converge; resync is bounded | Slice 2 property test over random publication sequences, with the encoded subset applied in order; slice 3b two-client test against a live PTY |
| A lost or misordered delta never corrupts rows | Slice 2 negative tests on `UpdateSeq` baselines; slice 3b scroll during a multi-frame update |
| Serialization never relies on `Arc` identity; metadata-only updates are visible | Slice 2 equal-content and metadata-only tests |
| Contract parity | Slice 3a and 3b scenarios run in process and over IPC, comparing results, digests, statuses, and rows, including an over-long grapheme |
| One host-effect recipient; no duplicate, loss, or replay | Slice 4 tests across embedded and IPC viewers, hidden viewers, a busy effect lane, permission revocation, disconnect, reconnect, and timeout |
| Version and capability negotiation | Slice 1 negotiation tests; vectors for every minor version; the test-only minor version run in both directions, newer server with minor-0 client and newer client with minor-0 server, against decoders (slice 1), updates and digests (slice 2), and a live hierarchy stream (slice 3a), with no resync |
| Reconnect reports what was missed | Slice 3b status-baseline carry-over test |
| Every state the limits permit is encodable | Slice 1 maximum-hierarchy test; slice 2 oversized-row, clamped-grid, and byte-split tests |
| Incompatible versions, malformed and oversized messages, stale incarnations | Slice 1 decoder and negotiation tests, slice 3a incarnation test, fuzz targets |
| Bounded memory per connection | Slice 1 stream, cap, and reassembly tests; slice 1 and 2 core invariant tests; slice 3a flood, slow-reader, stall, and undo-slot tests on the injected clock |
| Partial writes and slow or disconnected consumers | Slice 5 byte-split tests; slice 3a and 3b stall and disconnect tests |
| Retry behavior | Slice 3a tests for in-flight runtime queries, viewer requests, mutations, and shared setters; slice 3b refused-input resend test |
| PTY parsing and other viewers continue when a client stalls | Slice 3b stalled-client test that asserts another viewer's updates and PTY output progress |
| `wasm32-unknown-unknown` | CI check of `huterm-protocol`, `huterm-wire` with its vectors, and the sans-IO `huterm-client`; the vectors decode in native tests |
| Architecture boundaries | New `mise run architecture` rules, each shown to fail once against a deliberate violation |
| Peer authentication and socket permissions | Slice 5 tests, including a second-user test in Docker |

Property tests use `proptest` as a dev-dependency, synchronize on observable
progress, and bound their case counts so the normal suite stays fast. The
nightly fuzz job is evidence, not a gate; a crash becomes a regression test
in the normal suite. Two slices change existing native paths: slice 2's
cell-text cap, covered by a core engine test with an over-long grapheme, and
slice 3b's window-close change, covered by a core test and the existing
viewers and close smokes. Neither changes what the smokes observe, so no
smoke changes are needed until [#45][issue-45].

## Risks

- **Hand-written codec drift.** Exhaustive destructuring, golden vectors per
  minor version, round-trip property tests, and fuzzing cover it. The cost
  is more code than a derive.
- **Minor version discipline.** Additive evolution only works if each change
  is classified correctly and defines its downgrade and digest profile. The
  vector test catches byte changes, and the test-only minor version proves
  the mechanism; review must still catch a semantic change that needs a
  major version. Record the rule in a new `huterm-wire` section of the core
  runtime guide.
- **Cell text truncation.** Core truncating a grapheme over 1 KiB changes
  in-process rendering for pathological text. Real grapheme clusters stay far
  below that limit.
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
[issue-49]: https://github.com/jimeh/huterm/issues/49
[issue-80]: https://github.com/jimeh/huterm/issues/80
[issue-83]: https://github.com/jimeh/huterm/issues/83
[issue-88]: https://github.com/jimeh/huterm/issues/88
[issue-98]: https://github.com/jimeh/huterm/issues/98
[issue-131]: https://github.com/jimeh/huterm/issues/131
[issue-132]: https://github.com/jimeh/huterm/issues/132
[issue-133]: https://github.com/jimeh/huterm/issues/133
[issue-134]: https://github.com/jimeh/huterm/issues/134
