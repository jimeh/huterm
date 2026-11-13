# Local runtime IPC

Status: in progress for [issue #44][issue-44]. Slices 1, 2, 3a, and 3b are
implemented, with the revisions recorded under each; slices 4 and 5 remain.

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
  committed, or any input. In-flight requests whose outcome
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

The maintainer approved two further requirements on 2026-11-04. Both keep
open a later feature, a live handover, in which a running server passes its
PTYs to a newer server version so Huterm can be upgraded without ending
terminal programs. The handover itself is not part of #44 (see "Scope").

- **Identity survives a handover.** A handover keeps the server
  incarnation, and with it every session, workspace, tab, and terminal ID.
  The build and protocol versions are recorded separately from it. To a
  client, a handover looks like a reconnect followed by version
  renegotiation and a snapshot resync, not like a new server. Only a real
  restart, which loses the terminals, starts a new incarnation (see "Live
  handover stays possible").
- **Cross-version compatibility is the default.** Upgrading the GUI never
  requires restarting the server. A newer client works with an older
  running server, and an older client with a newer one, across every minor
  version of the same major version, and capability-gated features degrade
  as "Compatibility window" describes.

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
- The simulation proof in `crates/huterm-ipc-sim`, which checks the design
  itself under faults, version skew, and remote-like links.

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
| Live handover of a running server's PTYs to a newer server version | A later feature under [#18][issue-18], in its own issue |
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
  `portable-pty`, or `gpui-pre`, on any target, through normal or build
  edges, so its tests may still run against the real server.
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

- **Streams.** The sender picks a stream ID for each message from IDs it has
  no message in flight on; the first frame on a free ID starts a new message.
  Frames of different streams interleave, so a large snapshot cannot hold up
  a keystroke echo in another terminal. Each direction may have at most
  `MAX_OPEN_STREAMS` (4) unfinished messages; a sender waits rather than open
  a fifth, and receiving a fifth is a protocol violation. The server keeps
  one of its streams for host effects, whose frames its writer sends ahead
  of queued ones (see host effects); ordinary frames stay in order among
  themselves, so an ordinary stream ID is free again as soon as its
  message's last frame is built, and an effect never waits for a stream.
- **Ordering lanes.** Interleaving means messages on different streams can
  complete in any order, even single-frame messages, because the sender takes
  frames from its streams in turn. Every message therefore belongs to an
  ordering lane, and a lane holds at most one unfinished message, so a lane's
  messages complete in the order they started. The server's lanes are control
  (handshake replies and results), large results such as selection text, the
  hierarchy, host effects, and one lane per viewer for its updates, status,
  input acknowledgements, and its `ViewerStopped` or `ViewerRevoked`, which
  therefore follows the viewer's final status and update. The client's lanes
  are control (the handshake, and requests that do not name a viewer) and
  one per viewer for its input,
  credit, permission, focus, scroll, and resync requests, so a scroll stays in
  order with the typing around it. Lanes interleave freely. A viewer's lane
  opens only after the result that created the viewer has been queued in full,
  so the client always knows a viewer before its first update. The simulation
  proof found both failures this prevents: input arriving out of order, and a
  subscription's first update overtaking its result and being dropped.
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
  caps that bind the client. Senders always write their appended fields,
  and an older receiver skips them, so a minor version may append fields to
  a kind an older peer receives only if the kind's worst case, appended
  fields included, still fits the older version's cap. A newer sender
  therefore never pushes an older receiver over its cap. Projection applies
  to values and variants, such as a style or an event, never to appended
  fields.
- **Server memory.** The server holds at most `SERVER_REASSEMBLY_BUDGET`
  of unfinished client messages: `MAX_OPEN_STREAMS` times the largest client
  message cap, which is text input or a paste at its limit, so a little over
  4 MiB.
  `Welcome` advertises it with every cap that binds the client. A client
  waits rather than exceed it, so a well-behaved client never meets the
  limit, and exceeding it is a protocol violation.
- **Abort and idle.** A sender abandons an unfinished message with a frame
  that sets the abort flag, after the frame already being written, and the
  receiver frees the stream. The sender queues a large message's frames one
  at a time, each a copy of at most `max_frame` bytes, so abandoning a
  message releases its source, such as a host effect's reservation,
  immediately; only the frame already in the writer remains, and the
  writer-stall rule bounds how long that frame can take. The sender decides
  to abort by message identity, and sends an abort frame only while that
  message still owns its stream: once a message's frames are all queued, its
  stream ID is reused, and the simulation proof showed that aborting by
  stream ID kills whatever message took the stream over. Aborting a
  message whose frames are all queued does nothing, and that message still
  arrives. A stream that receives no frame for
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
  two peers' minor versions. A client closes the connection as a protocol
  violation if `Welcome` names another major version or a minor version
  above its own;
- the negotiated features: the intersection of both sets;
- the server incarnation, a stable server ID, the host name, the socket
  name, the server's build version, and its process ID. The server ID is
  generated once and kept with the socket scope's saved state, so a session
  picker or a saved client layout ([#42][issue-42]) can name the same server
  across restarts, while the incarnation changes only when the terminals
  are lost. The build version and process ID identify the process, which a
  handover replaces under the same incarnation; they are diagnostics, and
  no client behavior depends on them;
- the negotiated `max_frame` and the transport profile, which holds the
  limits and deadlines that bind the client, such as credit, in-flight
  requests, message caps, and the stall and effect deadlines (see "Bounds
  and backpressure").

`Reject` carries a reason code (`IncompatibleMajor` with the server's
versions, `ServerBusy`, or `Malformed`) and a diagnostic message. The
message kinds of `Hello` and `Reject`, and `Reject`'s reason and version
fields, keep their position and encoding in every major version, as
`Hello`'s leading fields do, so a client of another major version reads a
clear version error. Any message before `Hello` other than `Hello` itself,
or no `Hello` within `HANDSHAKE_DEADLINE` (5 s), closes the connection. A
client that has no `Welcome` or `Reject` within its own handshake deadline,
a transport-profile value longer than the link's round trip, closes and
reconnects, so a hung server cannot hold it forever.

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

**Compatibility window.** The window is the major version, in both
directions. A client speaks every minor version of its major from 0 up to
its own, and a server serves every client of its major. No minor version is
dropped within a major, because dropping one is itself an incompatible
change. A GUI updated by Sparkle therefore keeps working with the server
already running, and a server replaced by a later handover keeps serving
the older clients still attached, such as an older CLI. Only a major
version bump falls outside the window: the server answers `Reject` with
`IncompatibleMajor`, and until a handover can cross the bump, the user must
restart the server and lose its terminals. A major bump is therefore a last
resort (see "Risks").

Anything newer than the negotiated minor version, or outside the negotiated
features, degrades in one of three ways:

- State and events the server sends downgrade as described above, so the
  older peer always holds a complete, consistent state of its own version.
- A client never sends a request, ordered message, or variant the
  connection did not negotiate. Its library answers such a call locally
  with `Unsupported` and sends nothing; a server that cannot serve a
  request answers `Unsupported` too.
- The client disables the commands that need a missing minor version or
  feature and says why, for example that the command needs a newer Huterm
  server, rather than letting them fail when used.

Nothing negotiated outlives its connection. A reconnect, including one to
the same incarnation after a handover, may negotiate a different minor
version, feature set, `max_frame`, and transport profile, and the client
derives its caps, digest profiles, and enabled commands from the new
`Welcome` each time.

Golden vectors pin the encoding: `huterm-wire`'s `vectors` module holds
canonical values and their bytes per minor version. A test fails if an
existing vector's bytes change, so a released minor version cannot change
silently. A new minor version adds vectors.

The client kind is policy, not authentication. Authentication belongs to the
transport adapter, which gives each connection a peer identity: on local
IPC, `Peer::LocalSameUser` after the peer-credential check. Server policy,
such as host-effect eligibility and the defaults a client gets, reads the
peer identity and the client kind together, so a later authenticated remote
transport adds a peer variant instead of changing every policy check.

**Server incarnation.** `huterm-protocol` gains `ServerIncarnation`, 16
bytes the server reads from `/dev/urandom` at a cold start. It names one
lifetime of the runtime's terminals, not one process: a handover passes it
to the successor process, and only a start that loses the terminals, such
as a crash, a stop, or a restore from saved state ([#42][issue-42]), draws
a new one. It distinguishes those lifetimes across restarts and machines.
IDs on the wire are bare
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

**Live handover stays possible.** A later feature under [#18][issue-18]
will let a running server hand its live PTYs to a newer server version.
herdr's opt-in `herdr update --handoff` is prior art
([`src/server/handoff.rs`][herdr-handoff],
[session state][herdr-session-state]): it passes PTY master descriptors
over `SCM_RIGHTS`, checks a manifest of handoff, build, and protocol
versions, and commits a two-phase cutover in which the old server keeps its
panes if the handover fails. This plan does not design the handover, but
the contract must not rule it out:

- Everything the protocol promises within an incarnation is plain state a
  successor can inherit: the ID allocators for sessions, workspaces, tabs,
  terminals, and viewers, so no ID is reused within an incarnation; the
  hierarchy's committed sequence, so the `StreamId` and every sequence
  waiter stay valid; and each terminal's publication revision, status
  revision, bell count, and failure numbers, so carried status baselines
  stay correct. None of it may derive from process identity, such as a
  process ID, start time, memory address, or `Arc` identity.
- Connection state is not inherited. A handover ends every connection, and
  each client follows the ordinary reconnect rules: in-flight mutations
  resolve `OutcomeUnknown`, unacknowledged input may be lost but is never
  repeated, effects are never replayed, and viewers end with their
  connection. The client then reconnects to the same incarnation, keeps its
  ID mapping and status baselines, renegotiates the version, resubscribes,
  and installs fresh snapshots. Viewers are connection state on every
  reconnect, so only the viewer ID allocator crosses a handover.
- A client keeps three things per incarnation: its ID mapping, its status
  baselines, and the final views of stopped terminals. The status counters
  belong to minor version 0, so a renegotiated minor version never
  re-bases them. A stopped terminal's final view stays at the minor version
  it arrived in, because subscribing to a stopped terminal again returns
  only its status.
- The handover itself owns flushing input that the old server admitted but
  had not yet written to a PTY, passing the listening socket so the socket
  name stays the same, and any notice that tells clients to reconnect at
  once. Such a notice can arrive as a minor version; until then, clients
  treat the handover as an ordinary disconnect. It also decides whether a
  successor may be older than its predecessor, and what that successor does
  with state newer than its own version. The wire contract only requires
  that each connection receives state projected to its negotiated minor
  version.

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
- `committed`: the hierarchy sequence, and its digest while the connection
  is subscribed, for a structural request that committed before failing, as
  a failed close cleanup does today.
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
| Viewer request | Snapshot, scroll, selection text, link lookup, report geometry, submit presentation; focus, permission, and credit are ordered viewer messages that are likewise never retried | Not retried; fails with `Disconnected`. The old viewer is gone; the client subscribes a new one, sends its current state, and the new viewer's first update replaces any lost snapshot |
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

Input is not request-and-result, and it is never resent: input the client
sent but the server had not acknowledged when the connection broke may or
may not have reached the terminal, so a keystroke typed during a disconnect
can be lost but never duplicated (see input below). Host effects are never
resent or replayed.

Rejected: idempotency keys with a server-side dedupe table. Retrying a create
or close exactly once would need a client identity that outlives a
connection, plus retention and eviction rules, and the resynchronized state
already shows each outcome.

### Hierarchy over the wire

`SubscribeHierarchy` answers with the sequence it subscribed at, and the
server sends the current `HierarchyState`, its sequence, and its digest as
the first message on the hierarchy lane, ahead of every event that follows
it. The client resolves the request once its projection reaches that
sequence, through the same waiters as any structural result. The server
then forwards each envelope as a `HierarchyEvent { seq, event }` message on
the same lane. The connection supplies the stream, so the client rebuilds
the `StreamId`.

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
  Mux lock, by walking Mux's own structure, for each profile its
  subscribers registered. A connection's forwarder does not run a reducer of
  its own, so a reducer bug cannot produce matching digests on both ends.
  The walk costs time proportional to the hierarchy, under the Mux lock, for
  each registered profile, on every structural result, every subscription
  snapshot, and every 256th event. Hierarchies hold tens to hundreds of
  items, so this stays small; slice 3a measured it on a large fixture (see
  its revisions).

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
`MAX_VIEWER_CREDIT` (2) outstanding updates. Each update says whether it spent
a credit. The client returns a credit when it has disposed of a credited
update, whether it applied it or discarded it as stale, so a discarded delta
never leaks credit. Each credit return also names the last `UpdateSeq` the
client applied. Local IPC uses that acknowledgement only in assertions; it
lets a later remote transport keep a viewer's baseline across a short outage
and resume with a delta (see "Remote clients later"). The first update of a
subscription, the update that answers a scroll, the final update of a
stopped terminal, and the complete snapshot that answers `ResyncViewer` spend
none, so recovery never waits for credit. The
server's pump for a viewer waits on the viewer's wake and on credit grants;
when the viewer is notified and has credit, it requests a snapshot, encodes
the update, and spends a credit. A credit grant that raises a notified
viewer's credit from zero wakes the pump itself, so the update never waits for
another publication. A client grants no credit to a hidden tab, so its viewer
stays notified, the runtime does not wake it again, and a hidden terminal
costs the wire no row data until it is shown. This is the in-process rearm
rule carried across the wire.

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
incarnation, including across a handover, so the first status after
reconnecting reports an exit, bells, and failures that happened while the
client was away, and drives
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

**Revocation and stop.** When a terminal stops, the viewer's pump sends its
final status and a complete final snapshot that spends no credit, then
`ViewerStopped`, all on the viewer's lane, and stops. The snapshot is
complete, not a delta, because the client may be recovering from a damaged
delta whose resync request has not reached the server yet; a delta on a
discarded baseline would leave it a stale final view. The simulation proof
found this case. A
revoked viewer gets `ViewerRevoked` on its lane the same way, after
anything already queued there. A revoked viewer's requests answer
`Revoked`. The client keeps a stopped terminal's final status and last view
to show.

### Input

Input is not request-and-result. Each viewer numbers its input with a
client-assigned `InputSeq`, sends it on the viewer's lane so it stays in
order, and the server admits input from one viewer in that order.

- Core gives each remote viewer its own input byte budget,
  `VIEWER_INPUT_BUDGET` (1 MiB), passed through `ViewerOptions`. One
  connection's paste then cannot refuse another connection's keystrokes, as
  the viewers plan requires. Embedded viewers keep today's shared
  per-terminal budget.
- When core answers `Busy`, the server holds that viewer's input in an
  ordered staging queue, bounded by the same budget, and retries on the
  16 ms timer the desktop uses, armed only while input is staged.
- Input is flow-controlled by a window, so staging never overflows. The
  server acknowledges admitted input with `InputAck { viewer, seq }`, naming
  the highest sequence that reached core, and the client keeps its
  unacknowledged input within `VIEWER_INPUT_BUDGET`, queueing further typing
  locally, as the desktop's input queue does today. A client that exceeds
  the window commits a protocol violation. The simulation proof found that
  the earlier refuse-and-resume design (`InputRefused`, `ResumeInput`,
  `InputCapacity`) needs an unbounded resend buffer, because a client
  cannot tell which of its inputs the server will refuse; the window bounds
  what the client keeps to the budget, and unacknowledged input at a
  disconnect is never resent.
- A paste larger than one frame, up to exactly `PASTE_PAYLOAD_LIMIT`, is a
  multi-frame message that the server reassembles into one
  `TerminalInput::Paste` before admission, so bracketed paste stays atomic.
- Focus and geometry reports keep their in-process rules: focus is ordered
  with input and outside the byte budget, and geometry is latest-wins.

### Disconnect

A broken connection, a protocol violation, or a stalled writer ends the
connection. The transport profile sets a suspend grace period, during which
a client could resume the connection's attachments and viewers with a
resume token. Local IPC's grace period is zero, so the connection ends at
once, as follows; a later remote transport only changes the value:

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
| `VIEWER_INPUT_BUDGET` | 1 MiB of unacknowledged input per viewer | Client waits; exceeding it is a protocol violation |
| `HIERARCHY_LANE_BUDGET` per connection | 1 MiB | Lag, then `HierarchyResync` |
| `OUTBOUND_BUDGET` per connection | 8 MiB | Producers wait; inbound reading pauses |
| `WRITER_STALL_DEADLINE` | 30 s | Disconnect |
| `MAX_OPEN_STREAMS` per direction per connection | 4, one of the server's kept for host effects | Sender waits; receiving more is a protocol violation |
| `SERVER_REASSEMBLY_BUDGET` per connection, advertised in `Welcome` | `MAX_OPEN_STREAMS` times the largest client message cap, text input at its limit, about 4 MiB | Sender waits; receiving more is a protocol violation |
| `STREAM_IDLE_DEADLINE` | 30 s, paused while the server is not reading | Protocol violation |
| `CLIPBOARD_PAYLOAD_LIMIT`, `SELECTION_PAYLOAD_LIMIT` | 64 MiB of text each | Sender refuses with `LimitExceeded` |
| `PASTE_PAYLOAD_LIMIT`, `INPUT_TEXT_PAYLOAD_LIMIT` | `VIEWER_INPUT_BUDGET` | Sender refuses with `LimitExceeded` |
| `MAX_CHARACTER_TEXT_BYTES` | 256 bytes for a character chord's text and its layout text | Sender refuses with `LimitExceeded` |
| `EFFECT_RESPONSE_PAYLOAD_LIMIT` | 64 KiB | Sender refuses with `LimitExceeded` |
| `MessageCaps`, per kind and negotiated minor version | The kind's payload limit or core invariants plus its worst-case encoding overhead; 64 KiB for requests without a payload | Receiving more is a protocol violation |
| `MAX_GRID_CELLS`, a core invariant | 1,048,576 cells | Core clamps every grid it is given: spawn, split, resize, and geometry reports |
| `MAX_CELL_TEXT_BYTES`, a core invariant | 1 KiB | Core truncates at row extraction |
| Update message cap | From the grid an update's header declares: per cell, the cell-text limit plus its length prefix, run header, and style-table entry; per row and per update, their headers | Protocol violation |
| `MAX_NAME_BYTES`, a core invariant | 256 bytes | Custom names: `InvalidName`; fallback names: truncated |
| `MAX_HIERARCHY_ITEMS`, a core invariant | 16,384 sessions, workspaces, tabs, panes, splits, and attachments together | `LimitExceeded` from the structural operation |
| `MAX_LAYOUT_DEPTH`, a protocol invariant | 64 nodes from a layout's root to any pane | `PaneLayout::new` refuses; core refuses the split, move, or arrange |
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
output never exceeds `OUTBOUND_BUDGET` plus one ordinary frame, the last
one built before the queue reached its budget, and the effect lane's one
frame, however many viewers it has.

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
  `LimitExceeded` before it changes anything. Attachments count as items.
- A layout deeper than `MAX_LAYOUT_DEPTH` cannot be built: `PaneLayout::new`
  refuses it, so a split, move, or arrange that would pass it fails before
  it allocates an identity.
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

**Effect frames go first.** The lane has a stream of its own, so an effect
never waits for a stream. The writer sends the lane's next frame ahead of
queued update and result frames, and the lane reserves its own one-frame
slot rather than waiting on `OUTBOUND_BUDGET`. The slot refills as soon as
its frame is written, so the effect's next frame goes ahead of anything
queued, and a streaming effect gets the whole link. A busy connection
therefore delays an effect's first frame by at most the frame already in
the writer. The one exception is bounded: after a deadline aborts an effect,
its abort frame takes the slot, and the next effect starts once that frame
is written.

The lane pops a delivery, rechecks `is_current`, assigns a
connection-scoped `EffectId`, and sends `HostEffect { effect, viewer,
permission_generation, terminal, payload, response }`. The payload's
variant names its `HostEffectKind` (a clipboard write is `Clipboard`, a
notification is `Notification`), so the message carries no separate kind
field. Large payloads span frames like any message.

- **Permission travels with the effect, per kind.** In process, permission
  is per (recipient, kind) since [#81][issue-81]: each kind has its own
  allowed flag and generation, revoking a kind cancels that kind's queued
  and popped deliveries and leaves the others alone, and the desktop
  rechecks `is_current` immediately before the OS call (`host_effects.rs`,
  `HostEffectRecipient::set_allowed(kind, allowed)` and `is_current`, which
  compares the generation of the effect's kind). Over IPC the client owns
  each kind's permission and numbers its changes per viewer and kind.
  `SetHostEffectPermission { viewer, kind, generation, allowed }` is an
  ordered message, not a request, so it is never refused with `Busy`; the
  server applies it to that kind of the recipient and stamps each effect it
  admits with the generation it last received for the effect's kind. The
  client executes an effect only if that viewer is still live and the
  effect's generation equals its current generation for the payload's kind.
  An effect already sent when the user revokes its kind is discarded on the
  client, a revoke followed by a re-grant cannot revive it, and a change to
  one kind never invalidates effects of another.
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
  writer. A healthy client, on a link that sustains the profile's minimum
  throughput, therefore receives every effect, including one queued behind a
  maximum-size write. The throughput is a transport-profile value: the link
  must carry it plus the frame headers, or maximum-size effects miss their
  completion deadline and are dropped, so a profile for slow remote links
  lowers it. Because the slot refills at once, no further margin is needed;
  the first-frame allowance covers the frame in the writer and the bytes
  already in flight. Meanwhile a stalled remote recipient holds
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
connection's I/O or pump threads. Slice 3a picked `async-executor` with
`futures-lite`, `async-io`, and `blocking`, all already in `Cargo.lock` (see
its revisions).

### Remote clients later

Remote clients ([#19][issue-19], [#49][issue-49]) will let a desktop attach
to a Huterm server on another machine, as mosh and Eternal Terminal do for a
single shell. This plan leaves them out but keeps their path open. The
design already holds the property that makes those tools work: it
synchronizes state, not a byte stream. Eternal Terminal and Hubris replay
bytes because their clients run the emulator; mosh, like Huterm, sends the
current screen state and diffs against it.

What this plan provides for them:

- A contract over any reliable, ordered byte stream. An SSH channel to a
  proxy command, TLS, a WebSocket, or a QUIC stream can carry it unchanged.
- Correctness that never assumes low latency: ordering, gaps, and recovery
  are sequence and event driven, and latency only affects the transport
  profile's tuning.
- Five seams that are cheap now and costly to impose later: update
  acknowledgements on credit returns, a suspend grace period on disconnect,
  peer identity on every connection, limits and deadlines in a transport
  profile, and a stable server ID. The wire seams could arrive as minor
  versions too, but every client and server would then carry both paths,
  and the grace period and peer identity shape server internals that are
  cheaper to build that way from the start.

What waits for remote clients to be the focus:

- The transport. SSH first, as #49 prefers. QUIC is the candidate for
  roaming across network changes, because it keeps reliable, ordered
  streams. Mosh's unreliable datagram transport would need acknowledged
  diffs everywhere and is not planned.
- Authentication, host verification, and per-connection policy for remote
  peers, whose host effects stay ineligible until then.
- Transport integrity. The protocol relies on the transport to deliver
  bytes intact, as SSH, TLS, and QUIC do; a transport without integrity
  would need a frame checksum (see "Simulation proof").
- Liveness for half-open links, such as a keepalive message, which can
  arrive as a minor version.
- Resuming suspended connections, delta resume from acknowledged baselines,
  compression as a negotiated frame flag, local echo prediction in the
  client, and cycle checks across remote hops ([#47][issue-47]).

The simulation proof runs a remote-like link profile, with high round trip
times, jitter, frequent cuts, and daemon restarts, to check that the state
model and its bounds hold beyond a local socket.

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

## Simulation proof

`crates/huterm-ipc-sim` is a deterministic simulation of this design, kept
in the repository as its proof and as the place to try later design changes
before implementing them. It is not Huterm's implementation; slices start
fresh in `huterm-wire`, `huterm-client`, and `huterm-server`. It has no
normal or build dependencies, and nothing depends on it, which
`mise run architecture` checks. `mise run sim:ipc -- 2000` runs every
scenario over 2000 seeds, and `mise run sim:ipc -- remote 17` replays one
seed.

One single-threaded program holds a server, three clients, and a broker on
a virtual clock driven by a seeded generator, so every run replays exactly.
Messages are encoded and framed; the broker carries real bytes with
latency, jitter, random fragmentation, a bounded socket buffer, stalled and
trickling readers, connection cuts, and daemon restarts. Limits are scaled
down, with 48-byte frames and 4-by-6 grids, so multi-frame messages, caps,
and budgets are exercised constantly. It models the hierarchy and its
reducer, versioned digests and the three downgrade paths, appended fields
written at the sender's own version, terminals with shared rows and
revisioned status, stopped terminals' final status and view, host effects
with their lane, reservations, generations, deadlines, and shares, ordering
lanes, credit, lazy one-frame-at-a-time encoding, a writer that sends
effect frames first, the input window, retry classes, read-your-writes
waiters, and status baselines from a viewer's first read, carried across
reconnects.

| Scenario | What it adds |
| --- | --- |
| `clean` | No faults; every effect and input must arrive |
| `local-faults` | Cuts, stalls, trickle reads, 7-byte chunks, permission toggles |
| `remote` | 80 to 200 ms latency, 5-byte chunks, cuts, stalls, daemon restarts |
| `remote-healthy` | Remote latency with no faults; every effect must still arrive |
| `large-payloads` | Clipboard and selection payloads larger than the outbound budget |
| `newer-client` | Test-only minor 1 clients against a minor-0 server |
| `newer-server` | A minor-1 server against minor-0 and text clients |
| `state-corruption` | Damage that still decodes, injected into deltas and events on intact links; digests must catch every one |
| `handover` | Live handovers on healthy links: the server keeps its state and incarnation, alternates between minor versions 0 and 1, and ends every connection; minor-0 and minor-1 clients must renegotiate and carry their status baselines |
| `corruption` | Flipped and dropped bytes; only digested state is checked |
| `swarm` | Each seed draws its own limits, fault rates, link, client kinds, and versions |

After the faults stop, each run waits for quiescence and checks:

- every client's hierarchy equals the server's, projected to its version;
- every viewer's rows and metadata equal the terminal's current snapshot,
  and a stopped terminal's final view stays the client's;
- each client's status counts (bells, exit, failures) since its first read
  match the terminal's, including what happened while it was disconnected
  or during a handover;
- no intent runs twice, so no mutation is ever retried into a second run;
  a successful or committed result always ran, and a failed one never did;
- every effect runs at most once, on the connection core chose, and, on a
  healthy link however slow, every effect and input for a live terminal
  arrives, except on a connection a handover ended, where they may be lost
  as on any disconnect;
- input reaches the PTY once and in order per viewer;
- without damage, no client ever asks for a resync, and no connection ends
  for anything but a stall;
- on every tick, the outbound queue stays within its budget plus one
  ordinary frame and one effect frame, the server holds no encoded message
  outside it, and reassembly stays within its budget.

Each run also counts what it checked and which rare paths ran, and the tests
require each scenario to have compared real views and statuses and to have
exercised its faults, lag resyncs, `Busy` answers, subscriptions to stopped
terminals, whose final status is checked too, effect deadlines, all three
downgrade paths, in `state-corruption`, digests catching injected damage, and,
in `handover`, reconnects to the same incarnation that negotiated a different
minor version and viewers whose carried baselines reported what happened during
the handover. The server encodes each frame through a writer that keeps only
that frame's bytes, after a counting pass that sizes the message, so no whole
encoded message ever exists. A unit test checks every window against the full
encoding, and another checks that sizing stores no bytes and a window stores
only its frame. The simulation re-walks the message for each frame, which is
quadratic in its frame count; the real encoder should resume where the previous
frame ended.

The fixed scenarios each test one combination of limits. The `swarm` scenario is
a configuration fuzzer: every seed draws its frame size, stream count, caps,
budgets, credit, queue sizes, digest intervals, grid, fault rates including
handovers, link latency and fragmentation, and each peer's version and client
kind, within bounds that keep the configuration valid, such as deadlines longer
than the link's round trip. `mise run sim:ipc -- swarm 8` prints the
configuration seed 8 draws and replays it. On its first run the swarm caught a
model bug the fixed scenarios had hidden: with a status-text limit shorter than
the word "fatal", the stop path published an over-long failure message that the
client rejected, because that path skipped the truncation every other status
writer applied. The plan already requires core to apply each invariant on every
path that produces the value; the swarm is what checks it.

Each design rule the reviews argued over is load bearing: a test switches it
off and shows the simulation catches its absence. Without ordering lanes,
messages overtake each other; without carried status baselines, a
reconnect misses status; without versioned digests, older peers mismatch
while same-version peers still pass; retrying mutations after a reconnect
runs some twice; without digest checks, damage in `state-corruption`
diverges; without a complete final snapshot, a stopped view recovering
from damage stays wrong; a handover that draws a new incarnation makes
clients re-base their status and miss the exits, bells, and failures that
happened during it. Without credit on discard, a discarded delta leaks its
credit, which a client unit test shows directly, because with ordering
lanes no scenario discards a delta without corruption.

The simulation changed the design in six places, all reflected above:

- **Ordering lanes.** Messages on different streams complete in any
  order, so input arrived out of order and a subscription's first update
  could overtake its result and be dropped. A viewer's stop and revocation
  travel on its lane for the same reason, after its final status.
- **An input window.** Refuse-and-resume needed an unbounded resend buffer,
  because a client cannot know which inputs the server will refuse.
- **Abort by message.** An effect deadline that aborted by stream ID killed
  the viewer update that had reused the stream, silently freezing a view.
- **A stream of its own for effects.** With effect frames sent first at the
  writer, a stream ID reused as soon as its message's last frame was built
  let a new effect's frames overtake the old message's final frame on the
  same stream, and the receiver failed to decode. Holding every stream until
  its last frame was written fixed that but let an effect wait for a free
  stream behind the whole outbound queue, which broke its delivery deadline
  on healthy slow links. A stream reserved for effects fixes both: ordinary
  frames never reorder among themselves, and effects never wait.
- **A complete final snapshot on stop.** A viewer recovering from a
  damaged delta when its terminal stopped could receive the final update as
  a delta on the baseline it had discarded, then the stop, and keep a stale
  final view. The final update of a stopped terminal is now complete.
- **Transport integrity is assumed.** Digests catch damage to the
  hierarchy and rows that still decodes: `state-corruption` injects such
  damage on intact links and every run recovers by resync, while the same
  run with digest checks off diverges. Request IDs, effect identities, and
  status carry no integrity check, though, and a corrupted value there can
  still decode. Unix sockets, SSH, TLS, and QUIC all provide integrity; a
  transport without it would need a frame checksum. Periodic digests also
  find a corrupted delta only at the next digest, which is acceptable only
  because the transport does not corrupt. The byte-level `corruption`
  scenario reconnects only the clients whose links carried damaged bytes,
  and checks that recovery restores convergence.

Coverage-guided fuzzing of the schedule arrived with slice 1: the
`sim_schedule` cargo-fuzz target drives the simulation's random choices
from the fuzzer's input through `run_scheduled`, so it steers toward
interleavings that seeds rarely reach.

It does not model scrolling, mouse and focus arbitration, close tickets,
undo tokens, effects that need a response, pane layouts, the real codec's
encodings, sockets and peer credentials, the `wasm32-unknown-unknown`
build, or performance. Those stay with the slices.

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
  decoder targets, a target that drives the simulation's schedule from
  the fuzzer's input, a `fuzz:wire` Mise task on a pinned dated nightly that
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

#### Revisions during slice 1

The maintainer approved the first two through the coordinating thread on
2026-11-07, because they change the contract. The rest are implementation
choices within it.

- **Layout depth.** `PaneLayout` had no depth limit: alternating-axis splits
  nest without bound, because each new split has its own `SPLIT_TOTAL`. Its
  validation, the digest, and any decoder recurse per level, so a peer could
  send a cheap deeply nested layout that exhausts a thread's stack.
  `huterm-protocol` gains `MAX_LAYOUT_DEPTH` (64). `PaneLayout::new` refuses
  a deeper layout with `LayoutError::TooDeep`, and `check_split` refuses a
  split past it before core allocates, so core's splits, moves, and
  arrangements all hold it. The decoder checks the depth before it
  recurses.
- **Attachments are hierarchy items.** Snapshots carry every attachment, so
  the hierarchy cap needs them bounded. `MAX_HIERARCHY_ITEMS` counts them,
  and `attach_session` answers `LimitExceeded` at the limit.
- **Restores hold the invariants too.** Core refuses a capture with a name
  longer than `MAX_NAME_BYTES` (`InvalidName`) or more than
  `MAX_HIERARCHY_ITEMS` items (`LimitExceeded`), in `SpawnedTerminals::spawn`
  before anything starts and again in `Mux::restore`. The check stays out of
  `RuntimeCapture::validate`, because `huterm-state` shares that validator and
  keeps its own file bounds, such as 16,384 panes. `restore_tabs` refuses a
  recorded `LayoutChange` that holds a longer name with `InvalidRestore`.
- **Encodings.** `MAJOR` starts at 1. An enum whose variants carry data
  travels as a tag and a record, as the primitives table says; a field-less
  enum (split axis, client kind, retry class, error code, outcome) travels
  as a bare tag. Digests travel as 8 little-endian bytes, because a varint
  of a uniform 64-bit value usually takes 10. Message kinds are numbered
  `Hello` 0, `Welcome` 1, `Reject` 2, `HierarchySnapshot` 3,
  `HierarchyEvent` 4, `HierarchyResync` 5, `Paste` 6, and `HostEffect` 7,
  and the handshake kinds' cap is `HANDSHAKE_FRAME_LIMIT`. That cap is frozen
  for `Hello` in every major version, with its kind and leading fields, so a
  server of any major version reads a newer client's `Hello` and answers
  `Reject` instead of closing on an over-cap message.
- **Payload records ahead of their slices.** The acceptance criteria need a
  paste and a clipboard write at their limits, so slice 1 defines the
  `Paste` and `HostEffect` records. A paste is its own kind, so only it
  carries the paste cap. A host effect's `permission_generation` counts the
  generation of its payload's kind, which `EffectPayload::kind` maps onto
  #81's `HostEffectKind`; the notification payload arrives with slice 4.
  Minor version 0 is unreleased until #45 ships a server, so slices 3b
  and 4 may still change these records, as later slices add kinds and error
  codes.
- **The test-only minor version** is `MINOR + 1`, behind the `test-minor`
  feature. It appends a tab's pinned flag and adds `TabPinned` (sent to
  minor 0 as `Skipped`), `TabRetitled` (as `TabRenamed`), and
  `TabsExchanged` (as a projected `HierarchyResync`). Slice 1 proves the
  downgrade at the codec; slice 3a reuses the same events on a live stream.
  The cell style flag arrives with slice 2's update encoder.
- **New protocol variants need a wire form.** `HierarchyEvent` is
  `non_exhaustive`, so a variant added to `huterm-protocol` compiles in
  `huterm-wire` but encodes as `Unsupported` until it gets a tag, an
  encoding, and a golden vector. The vector test counts every variant.
- **Fuzzing.** `fuzz/rust-toolchain.toml` pins `nightly-2026-11-03`, and
  `fuzz:wire` installs `cargo-fuzz` 0.13.2 as a task tool. The `Fuzz`
  workflow runs daily and on dispatch, and no ruleset requires it. The
  schedule target drives `huterm-ipc-sim` through `run_scheduled`, which
  draws the scheduler's choices from the fuzzer's bytes before its seed.

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

#### Revisions during slice 2

None of these changes a settled decision; each is an implementation choice
within the contract, or a narrow correction to its text.

- **Ghostty bounds graphemes below the cell text limit.** The pinned Ghostty
  keeps a cluster's first codepoint and at most 64 more
  (`grapheme_max_len` in `src/terminal/page.zig`), so a cell holds at most
  260 bytes and core's cut at `MAX_CELL_TEXT_BYTES` guards a pin that raises
  that bound. No grapheme can reach the limit through the engine today, so
  the core test pins Ghostty's bound through the engine and checks the
  extraction helper's cut directly; the wire tests encode rows at the limit.
- **The grid opens an update's record.** A receiver needs the grid to know
  an update's cap, so `columns` and `rows` are the record's first fields.
  The reassembler reads them with the kind and length from the bytes so far
  and the arriving frame's first bytes, and checks the cap before it buffers
  that frame, for every kind; it used to append the frame first. The decoder
  checks the whole message against `MessageCaps::update_cap` for that grid.
  The kind's own cap is the largest any grid allows, about 1.1 GiB at
  `MAX_GRID_CELLS`, which only a worst case near the cell cap approaches.
- **One clamp for every grid.** `GridSize::clamped` applies
  `MAX_GRID_CELLS` as well as its one-cell minimum, shrinking both
  dimensions by one factor so the aspect stays. Geometry reports, initial
  viewer geometry, and PTY sizes already went through it, and
  `TerminalRuntime::spawn_in` now clamps a command's grid past the cap, which
  covers spawns and splits. A smaller grid reaches the engine unchanged, so
  an empty one still fails initialization before the child starts.
- **Style flags.** A style-table entry is a foreground, a background, and a
  flag bitset. Minor version 0 defines the eight `CellStyle` flags in
  declaration order, and a decoder rejects any other bit, so a sender
  projects a newer flag onto an older peer through
  `UpdateRow::flags(column, minor)`. The test-only minor version's flag is a
  curly underline, projected as a plain underline. Runs are packed, a style
  index and then a sequence of cell texts, while full rows and style entries
  are records so a later minor version can append fields; the cap reserves 4
  bytes for each.
- **Every update carries the default background.** The list of non-row
  fields above omits `default_background` (OSC 11), which
  `TerminalSnapshot` carries; every update carries it and the digest covers
  it.
- **The view digest lives in `huterm-wire`.** `update::view_digest` is
  FNV-1a over a canonical encoding that is independent of the codec, and it
  hashes each cell's flags as `UpdateRow::flags` projects them for the
  negotiated minor version. Unlike the hierarchy digest, core never computes
  it: the server's encoder and the client's view both sit on
  `huterm-wire`. `vectors::SAMPLE_VIEW_DIGEST` pins the encoding.
- **Stale updates.** A client discards any update whose number is not above
  the last one it applied, complete or delta, without a resync, so a
  duplicated or late complete snapshot never rolls the view back. A delta
  applies only when it names exactly the last applied update. A delta whose
  digest differs is still installed, as the best view the client has, while
  the client waits for the snapshot that answers its one `ResyncViewer`. A
  complete snapshot without a digest is a protocol violation, so a client can
  always tell a contract fault, and the client computes a digest only for an
  update that carries one.
- **Encoders refuse what decoders reject.** `encode_update` and
  `encode_status` repeat the decoders' structural checks: sequence numbers,
  `Keep` indices, empty runs, style indices, the complete snapshot's digest,
  log order, and progress percentages. A sender then refuses a malformed
  value locally instead of ending the connection. Times encode down to the
  earliest a decoder accepts, `i64::MIN` seconds, so every decoded status
  re-encodes.
- **Viewer messages.** Slice 2 adds the kinds `TerminalUpdate` 8,
  `ViewerStatus` 9, `Input` 10, `InputAck` 11, `Credit` 12, and
  `ResyncViewer` 13. `Paste` gains the `InputStamp` that `Input` carries,
  after its sequence. `Credit` grants 1 to `MAX_VIEWER_CREDIT` credits and
  names the last applied update. Text input is bounded by
  `INPUT_TEXT_PAYLOAD_LIMIT`, the viewer input budget, as a paste is, so
  `Input` is now the largest client message; a character chord's texts are
  bounded by `MAX_CHARACTER_TEXT_BYTES`. Focus, scroll, and geometry stay
  with slice 3a's request inventory and slice 3b.
- **Status text covers command records and metadata.**
  `MAX_STATUS_TEXT_BYTES` also bounds each command record's text, which core
  already cut at 4096 bytes, and every metadata string: directory hosts and
  paths and the foreground process and title. The status cap includes the
  256 command records. Times travel as signed seconds and nanoseconds from
  the Unix epoch. A decoder rejects failures or command records out of
  increasing sequence order, which `StatusBaseline` and the record lookups
  assume.
- **Generic rows.** `UpdateEncoder` and `ViewerView` are generic over the
  rows they encode and hold (`UpdateSource`, `SourceRow`, `ViewRow`), with
  `Arc<TerminalSnapshot>` and `Arc<TerminalRow>` as the defaults, so the
  test-only minor version reuses them with its own rows.
- **Core's grid test encodes.** `huterm-core` gains a dev-dependency on
  `huterm-wire`, so its grid test encodes the first update of a clamped
  spawn, split, and resize. Its normal dependencies are unchanged.
- **The worst case at the cell cap is measured, not built.** A worst-case
  update at `MAX_GRID_CELLS` is over 1 GiB. The test encodes its header with
  every row kept, then encodes each full row and its style entries one row
  at a time and sums the lengths, so the whole message never exists.

#### Wire performance pass

A follow-up to slice 2 settled the update format's costs while minor
version 0 is unreleased, before slices 3a and 3b build on it. None of these
changes a settled decision. `mise run bench:wire` measures the codec on
synthetic plain, styled, shell-like sparse, and CJK wide-character screens
at 80x24, 200x60, and 400x120, and `ci:benchmarks` gates it. The changes
replace the slice 2 text above where they differ:

- **The view digest hashes words.** `view_digest` feeds a canonical
  sequence of 64-bit words through `MurmurHash64A`'s word step and
  finalizer instead of feeding bytes through FNV-1a. A cell takes two words
  in the common case: its colors and low style flags, then its text length
  with up to 6 text bytes. Longer text adds zero-padded words, and flag bits
  above 14 add a word that the length word marks. Counts precede what they
  count, so distinct views give distinct sequences. Every step is a
  bijection of the state for a fixed word and of the word for a fixed
  state, so a change to any single word always changes the digest, which
  covers the decodable damage `state-corruption` injects. The digest still
  covers exactly the negotiated minor version's fields, builds its words
  arithmetically so every byte order agrees, and needs no dependency.
  `vectors::SAMPLE_VIEW_DIGEST` changed with it.
- **Runs have four forms.** A run opens with one varint holding its style
  index and two form bits. A *texts* run keeps slice 2's length-prefixed
  text per cell. A *packed* run holds cells that each have no text or one
  scalar other than U+0000, as one UTF-8 string with U+0000 standing for no
  text, so no text stays distinct from a written space. A *fill* run is
  identical packed cells as a count and one packed cell; the encoder fills
  only repeats of at least 8 cells, and decoders accept any count from 1. A *wide*
  run alternates wide characters and their spacers: it carries only the
  characters, and each spacer has no text, the same colors, and the spacer
  flag in place of the wide flag. Ghostty writes spacers that way, and a
  spacer that differs travels in an ordinary run. Encoders pick the form
  from the run's cells, and decoders accept any form for any run whose
  cells fit it. Decoders keep every slice 2 check and add their own: a
  packed string's byte length and scalar count against the row's remaining
  cells, a fill's count and its single scalar, and a wide run's style.
  Packed, fill, and wide cells decode to inline cell text, so decoded memory
  stays bounded by the grid.
- **Kept rows travel as ranges.** A row operation is a range of consecutive
  baseline rows (start and count) or a full row, so a scroll costs one
  operation for every kept row. Decoders bound the expanded row count at
  every operation.
- **Caps.** The run header's two form bits raise its worst-case varint by
  at most one byte, and `update_record_cap` uses it. A packed or wide cell
  costs at most four bytes, and a fill at most a few bytes for any number of
  cells, so the worst case remains a texts run per cell at
  `MAX_CELL_TEXT_BYTES`, and the worst-case tests still hold.
- **In-memory shape.** A `FullRow` holds its cells' texts in one vector,
  and each `Run` holds a style, a wide flag, and a cell count, so neither
  side allocates per run. The encoder looks up a style only when it differs
  from the previous cell's, through a hand-written hasher, and
  `Writer::record` reserves room for the length prefix instead of splicing
  the record body.

Medians on an Apple M3 Max at 400x120, interleaving five runs of the base
and of this change. Server time is building and encoding an update; client
time is decoding and applying it, digest check included.

| Screen | Update | Bytes before | Bytes after | Server before | Server after | Client before | Client after |
| --- | --- | --- | --- | --- | --- | --- | --- |
| plain | complete | 97,242 | 49,242 | 1.80 ms | 0.66 ms | 1.10 ms | 0.38 ms |
| styled | complete | 108,958 | 60,958 | 2.39 ms | 0.76 ms | 1.34 ms | 0.48 ms |
| sparse | complete | 60,942 | 13,391 | 1.79 ms | 0.66 ms | 1.08 ms | 0.39 ms |
| wide | complete | 217,007 | 73,242 | 2.91 ms | 0.84 ms | 2.12 ms | 0.54 ms |
| plain | one row | 1,082 | 450 | 15 us | 6.5 us | 4.2 us | 2.2 us |
| sparse | one row | 704 | 74 | 14 us | 6.2 us | 4.2 us | 2.4 us |
| wide | one row | 2,085 | 650 | 24 us | 7.6 us | 15 us | 3.0 us |

The view digest fell from about 0.75 ms to about 0.19 ms on a plain screen.
A window-edge resize drag at 400x120, one complete snapshot per geometry
change, now costs about 1.0 ms of combined server and client work for plain
text and 1.4 ms for wide text, against 2.9 ms and 5.0 ms before. Slice 3b
still owns encoding straight from the snapshot without a `TerminalUpdate`
value or a whole-message buffer, and the end-to-end benchmark through a
remote viewer.

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
- A reconnect to the same incarnation that negotiates a different minor
  version, as after a handover, keeps every ID valid and every sequence
  waiter live, and the client takes its caps and digest profile from the
  new `Welcome`.
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

#### Revisions during slice 3a

None of these changes a settled decision; each is an implementation choice
within the contract, or a narrow correction to its text.

The wire inventory:

- **Request kinds.** Slice 3a adds the message kinds `Request` 14, `Result`
  15, and `Cancel` 16. A `RequestId` is a per-connection `u64` that starts
  at 1, is never reused, and is never 0, which decoders reject. Each
  `RequestKind` has a stable tag, a static `RetryClass` (`Query`, `Viewer`,
  or `Mutation`), the minor version that introduced it, and a
  `large_result` flag that no slice 3a kind sets. The queries are
  `SubscribeHierarchy`, `ResyncHierarchy`, the four selections, and
  `PrepareClose`; `ReportGeometry` is the only `Viewer` request, and every
  other request is a `Mutation`. `ToggleInputSync` and
  `CloseEmptiedWorkspace` mirror the desktop's composite calls.
  `ExecuteCommand` carries a `DecodedInvocation`, so an identifier the
  server's catalog does not know answers `UnknownCommand` with its text.
  `Request` and `RequestKind` are `non_exhaustive`, because the test-only
  `PinTab` request exists whenever any crate in a build enables
  `test-minor`; a server answers a variant it cannot serve with
  `Unsupported`, and a minor-0 encoder refuses `PinTab` the same way.
- **Viewer requests wait for slice 3b.** Slice 3a defines only
  `SetScrollback`, `ClearHistory`, and `Reset` (class `Mutation`) and
  `ReportGeometry` (class `Viewer`), so the retry classes and their tests
  are real; its server holds no viewers and answers them `StaleTarget`, and
  drops the ordered viewer messages (`Input`, `Paste`, `Credit`, and
  `ResyncViewer`). Slice 3b settles the semantics of the remaining
  `TerminalViewer` operations and adds them: `subscribe_terminal`,
  `request_snapshot`, `request_scrolled_snapshot`,
  `request_snapshot_with_link`, `request_selection`,
  `request_select_units`, `update_presentation`,
  `set_scrollback_compression`, `set_focus`, and input.
- **Where a result committed.** `WireError.committed_seq` became
  `committed: Option<Committed>`, where `Committed { seq, digest }` travels
  as a record and carries the digest while the connection is subscribed.
  Every structural success carries the same field, so the client checks a
  digest at every structural result. The committed-error vector changed
  with it. A new code, `Cancelled` (20), answers
  a request cancelled before it started; it is safe to retry, because
  nothing committed, and `WireError::new` marks only `Busy` and `Cancelled`
  safe.
- **Caps.** The `Request` cap is 628,872 bytes: an `ArrangePanes` whose
  expected layout and arrangement each hold `MAX_REQUEST_LAYOUT_ITEMS`
  (1,024) items with every name at its limit. A launch is bounded by
  `MAX_LAUNCH_PATH_BYTES` (4,096), `MAX_LAUNCH_ARGUMENTS` and
  `MAX_LAUNCH_ENVIRONMENT` (1,024 each), and a 256 KiB
  `LAUNCH_PAYLOAD_LIMIT` that arguments and environment share, and a close
  names at most `MAX_CLOSE_REQUEST_TABS` (1,024) tabs. `Input` stays the
  largest client message, so `SERVER_REASSEMBLY_BUDGET` is unchanged. The
  `Result` cap is 52,788 bytes and fits one frame at the 64 KiB minimum
  `max_frame`, including a close assessment of at most 32 busy terminals
  and 32 processes, with commands cut to 256 bytes and command lines to
  1,024, plus counts of what it left out. Request names may be up to
  `MAX_REQUEST_NAME_BYTES` (1,024), so core, not the codec, answers
  `InvalidName` for a name over 256 bytes, as in process. `Welcome` lists
  the two new client kinds' caps, which changed its minor-0 vector.
- **Wire forms.** Paths travel as `OsStr` bytes on Unix and as UTF-8
  elsewhere. Shell features travel as a bit set, and a feature without a
  bit encodes as `Unsupported`. A presentation travels as raw RGB bytes,
  all 256 palette entries included. Arrangements check their depth before
  recursing, at most `MAX_SPLIT_CHILDREN` children per split, and their
  node count.

Digests at emission:

- **One canonical encoding.** It moved into `huterm_protocol::HierarchyDigester`,
  which `HierarchyState::digest_for` uses, so `SAMPLE_HIERARCHY_DIGEST` is
  unchanged. `HIERARCHY_DIGEST_INTERVAL` (256) moved to `huterm-protocol`
  with it. `Mux::hierarchy_digest(profile)` walks Mux's own structure under
  the caller's lock, building no `HierarchyState` and copying no name.
  `Mux::subscribe_hierarchy_with_digests(profile)` returns the snapshot,
  its digest, and a subscription whose events carry digests, computed once
  per distinct registered profile at each interval multiple, after the
  change applies. `subscribe_hierarchy` computes none, so embedded mode
  pays nothing.

Release medians on an Apple M3 Max
(`cargo test --release -p huterm-core hierarchy_digest_cost -- --ignored`):

| Fixture | Items | `Mux::hierarchy_digest` | Build a state, then digest | Digest a built state |
| --- | --- | --- | --- | --- |
| Small, 8-byte names | 21 | 1.0 us | 3.3 us | 1.0 us |
| Small, 256-byte names | 21 | 8.8 us | 11.3 us | 8.9 us |
| Large, 8-byte names | 15,552 | 0.79 ms | 2.94 ms | 0.79 ms |
| Large, 256-byte names | 15,552 | 7.27 ms | 9.81 ms | 7.34 ms |

The server:

- **A sans-IO connection.** `huterm-server`'s `Connection` is the whole
  per-connection protocol as a deterministic state machine. It takes bytes
  in, hands out frames, and runs on an injected clock. It hands out `Work`
  and takes back each `Completion`, and never touches the `Mux`;
  `runtime::execute` runs a job against the shared `Mux` on a worker. Work
  and completions are plain values that carry what they hold, such as a
  ticket's consent or a history record, so a test, or slice 3b's seeded
  fault-injection harness, can hold, reorder, or fail them and drive the
  connection with any transport and clock. Closing returns the disconnect
  cleanup, and a completion that arrives after a close releases what its
  job created through cleanup work.
- **The executor.** Each connection runs a reader, a writer, and a driver
  thread. The driver runs the connection on an `async-executor`
  `LocalExecutor` under `futures_lite::future::block_on`, waiting on one
  event channel, which the reader, the writer, worker completions, and a
  forwarding task for the hierarchy feed feed, or on an `async_io::Timer`
  for the next deadline. Jobs run through `blocking::unblock`, so slice 3b
  adds one forwarding task per viewer wake. The reader reads only after the
  driver grants a permit while `wants_input()` holds, so at most one read
  buffer is in flight. The writer writes one batch at a time with vectored
  writes straight from the shared message buffers. Only a batch's first
  frame is cut short to fit the batch; a later frame that does not fit
  whole waits for the next batch, so a message that fits one frame is
  never split across two. The event channel is
  unbounded, but each source is bounded: one read, one batch, the in-flight
  jobs, and one coalesced wake per feed. At the end, the driver shuts the
  socket down so blocked threads return and waits for every outstanding job
  and its cleanup.
- **The retained-bytes bound.** Each message is encoded once, when it is
  admitted, into a buffer the queue and the writer share; frames are cut
  from it in place. Producers admit messages only while retained bytes are
  under `OUTBOUND_BUDGET`, and at the budget the connection also stops
  reading and arms the over-budget stall deadline. A replacement snapshot
  is issued only under the budget and admitted when it completes. A
  connection therefore retains at most `OUTBOUND_BUDGET` plus one hierarchy
  message plus one result or hierarchy message
  (`ServerConfig::retained_bound`). Every hierarchy kind shares one computed
  cap of 9,617,517 bytes, so the default bound is 27,623,642 bytes. The
  largest snapshot measured is 8,725,140 bytes, for tabs of full splits, so
  with real event sizes a connection retains about 17 MiB at worst.
  Without a hierarchy subscription only results flow, so a connection
  retains at most `OUTBOUND_BUDGET` plus one result at its cap, which the
  native flood test asserts.
- **One replacement in flight.** A subscribe, a client resync, and a lag
  resync each replace the hierarchy feed, and at most one replacement runs
  per connection, so two replacement jobs never coexist. The contract's
  check that two replacements completing in reverse order never roll the
  client back becomes: a late completion for a superseded replacement is
  dropped and never rolls the client back.
- **Close tickets and attachments.** An unknown, consumed, or released
  ticket answers `StaleTarget`; `StaleClose` stays core's revision check.
  Releasing a ticket that a job holds answers `Unit` and drops the record
  when the job completes; until then, a commit or another release of it
  answers `StaleTarget`, as for any released ticket. `AttachSession` makes
  the attachment the connection's. `DetachSession`, `RetargetAttachment`,
  and a `PrepareClose` naming `Window` or `EmptiedWindow` accept only the
  connection's own attachments and otherwise answer `NotPermitted`.
  `AttachSession`, `CommitClose`, and `CloseSession` carry the connection's
  attachments and report which of them no longer exist, read under the
  lock, and the connection drops those. Another caller can end the
  connection's attachments at any time, even between an attach job and its
  completion, so the connection's set holds at most its live attachments
  plus those ended since the last such job, and repeated attach and
  external close cycles never grow it. A disconnect detaches the rest on a
  worker, and an attachment a late job creates is detached by cleanup
  work.
- **History slots.** A rearrangement touches its slot when its job starts,
  which creates it or evicts the least recently used slot and records the
  slot's generation. On completion, the change is filed as the undo entry
  and redo is cleared, unless the slot's generation changed, the change is
  empty, or it committed before the slot's newest sequence. A token names
  one entry in one direction. A restore takes its entry out; on success it
  files the same change the other way with a new token, and on failure it
  drops the entry. Records count while they live in the table or in a job.
- **Request IDs.** The server records used IDs as a floor plus the IDs
  seen above it. An ID at or below the floor, one seen before, or one more
  than `REQUEST_ID_WINDOW` (1,024) above the floor is a protocol violation,
  which keeps the record bounded and lets slice 3b's viewer lanes deliver
  requests out of number order.
- **Errors.** `runtime::error_code` is the one mapping from core's errors:
  `ForeignRuntime` maps to `StaleTarget`, as a client refuses a foreign
  identity locally, and `LayoutChanged` and `InvalidRestore` map to
  `StaleUndo`. `Welcome` advertises `ServerConfig::max_in_flight_requests`,
  the value the server enforces.
- **The hierarchy feed seam.** A `HierarchySource` captures a snapshot and
  a new feed under one lock; the snapshot encodes later, off the lock, and
  the feed hands out each event already encoded for its connection.
  Production uses `CoreSource`. The test-only `TestSource` applies
  `TestEvent`s and delivers each at the peer's minor version, so a test
  that uses it drives the hierarchy only through it.

The client:

- **A sans-IO session.** `huterm-client`'s `ClientSession` is the whole
  client protocol as a deterministic state machine that lives across
  connections, and it builds for `wasm32-unknown-unknown`. On Unix,
  `native::Client` runs it with a reader and a writer thread per connection
  and one driver thread, which waits on one event channel or an
  `async_io::Timer` under `futures_lite::future::block_on`, so the client
  needs no executor. When a connection ends, the driver shuts the socket
  down and joins both I/O threads before it reports the end. The transport
  never reconnects on its own: `ClientHandle::reconnect` takes a new
  stream, reports the old connection's end, and resolves with the new
  handshake; slice 5 adds `connect`. `ClientHandle::events` holds at most
  `EVENT_CHANNEL_CAPACITY` (1,024) events, and the driver never waits for
  its consumer: while it is full, events are dropped, and once there is
  room the consumer gets one `ClientEvent::Lagged` and re-reads the
  projection, as after core's subscription lag. That re-read is not atomic
  with the channel, as core's resubscribe is: events queued behind the
  marker may already be in the re-read projection, so the consumer skips an
  `Applied` event at or below the re-read sequence. The driver keeps the
  last connection's close reason, and `ClientHandle::connected` returns it
  while no connection is open, so a dropped `Closed` loses no `Rejected`,
  `ContractFault`, or timeout. Results, waiter resolutions, and handshake
  outcomes travel on their own reply channels.
- **Calls outlive connections.** A caller's request has a session-wide
  `CallId`, and each connection numbers what it sends from 1, as the
  server's request window requires, so a query resent on the next
  connection keeps its call and gets a new request number.
- **Unsent requests at a disconnect.** A mutation sent on the broken
  connection resolves `OutcomeUnknown`. A mutation never sent resolves
  `Disconnected`, because it did not run: one queued during a handshake,
  one held past the in-flight limit, or one issued with no connection.
  Viewer requests resolve `Disconnected`. A sent query the caller
  cancelled resolves `Cancelled`. Other queries wait for the next
  connection and go out in issue order. `abandon`, and shutting the native
  client down, fail the waiting queries with `Disconnected` and cancel
  every sequence waiter.
- **The client subscribes because the caller did.** The session subscribes
  only after a `SubscribeHierarchy` of the caller's succeeds, and from then
  on every connection subscribes again by itself; a cancelled or failed
  subscribe changes nothing, and a caller's subscribe that waits for the
  next connection goes out there instead of a second one. A
  `SubscribeHierarchy` or `ResyncHierarchy` result is held until the
  projection reaches its sequence.
- **Digest checks.** A snapshot whose digest disagrees with its state
  closes the connection as a contract fault. A mismatch on a digested event
  or at a structural checkpoint, a reducer gap, or an inconsistent event
  sends one `ResyncHierarchy`; at most one is outstanding. A result's
  committed digest is checked at once when the projection holds that
  sequence, kept and checked right after that event applies when the
  projection is behind, and dropped when the projection has passed it. An
  installed snapshot drops the checkpoints it covers, unchecked. A result
  frees its request slot while its checkpoint waits, so in-flight requests
  do not bound checkpoints; `MAX_PENDING_CHECKPOINTS` (4,096, four times
  core's 1,024-event subscription queue, past which the server lags and
  resyncs) does. Past it, the oldest checkpoint is dropped and counted in
  `SessionStats::checkpoints_dropped`, and the check moves to a snapshot:
  the first drop since a snapshot last covered one sends a
  `ResyncHierarchy` under the same at-most-one rule. That resync covers the
  drop, because the server committed the dropped sequence before the client
  read its result. Later drops only raise the sequence a snapshot must
  reach, so a flood of results sends one resync, not one per result. A
  drop while another resync is outstanding can outrun that resync's
  snapshot; once the snapshot installs and its resync succeeds, the session
  sends one more. A failed resync sends nothing more, and a drop before
  the connection's first snapshot waits for it. A snapshot
  behind the projection is a violation only after a snapshot installed on
  the same connection, and a hierarchy event before its connection's first
  snapshot is a violation.
- **The transport profile and the client's deadlines.** The client takes
  `max_in_flight_requests` from `Welcome` and holds requests back past it;
  its request and cancel caps are the smallest of its own cap, the
  advertised cap, and the advertised reassembly budget. It refuses, as a
  protocol violation, a profile it cannot honour: a nonzero suspend grace,
  no requests in flight, or no cap for requests or cancels. The handshake
  and progress deadlines bound how long the client waits on the server, so
  they live in `ClientConfig` as `CLIENT_HANDSHAKE_DEADLINE` (5 s) and
  `CLIENT_PROGRESS_DEADLINE` (30 s), not in `Welcome`. The progress
  deadline is armed while a sent request is unanswered, a sequence waiter
  exists, or a subscription result waits for the projection; every inbound
  frame restarts it.
- **Waiters.** `wait_for` resolves `Ready` at once when the projection
  already holds the sequence, and `Cancelled` at once before any connection
  has named a stream. A new incarnation cancels every waiter, and the same
  incarnation keeps them across connections.
- **The outbound path.** Every client message travels on the control lane
  and stream 0, and its frames are written in full before the next
  message's. A message is encoded once, and the native writer writes
  batches of up to about 256 KiB with vectored writes, so no payload is
  copied before the socket. Per request, the client allocates the encoded
  message, its call-table entry, and, natively, its reply channel.
- **The test-minor projection.** Under `cfg(any(test, feature =
  "test-minor"))`, `ClientSession::with_test_projection` sends `Hello.minor`
  as `TEST_MINOR` and holds a `TestHierarchy` projection that computes
  digests at the negotiated minor version. Production code holds only the
  minor-0 projection.
- **Gates.** `mise run architecture` checks `huterm-client`'s normal and
  build edges with all features on every target, so its tests may
  dev-depend on `huterm-server` and `huterm-core` and run against the real
  server. `mise run check:wasm` checks `huterm-client`.

Contract tests and measurements:

- **The contract harness** lives in `huterm-server`'s unit tests
  (`contract_tests.rs`), which reach the server's test support through
  `cfg(test)`; an integration target of the same crate would need a
  dev-dependency on itself to enable it. Its in-process path calls `Mux`
  directly, maps errors through `runtime::error_code`, and keeps close
  tickets and layout history with the server's semantics over core's own
  values, using its own conversions so a slip in the server's shows up as a
  difference. Its IPC path runs the native adapters over a `UnixStream`
  pair. Every step compares results, with identities by number and tokens
  and tickets by first appearance, error codes and retry, committed
  positions, and then the hierarchy's sequence and digest, from
  `Mux::hierarchy_digest` in process and from the client's projection over
  IPC.
- **IPC latency.** `mise run bench:ipc` reports a `RenameTab` round trip
  over the native adapters, the latency from a rename committed on the
  server's `Mux` to the client's projection applying its event, and the
  same rename in process. It gates nothing, because no CI runner has
  measured it yet. A round trip takes ten thread wakeups: the client's
  driver and writer; the server's reader, driver, worker, driver again, and
  writer; and the client's reader, driver, and caller. They take most of
  its time, since the rename itself takes 0.04 us and the digest about
  1 us. Reusing the server reader's buffer instead of copying each read
  moved the round trip's median by less than its run-to-run spread, so the
  reader keeps its copy.

Five runs of 5,000 samples after 5,000 warm-up samples, in release on an
Apple M3 Max, with a one-tab hierarchy; each cell gives the range across
runs.

| Measurement | Median | 99th percentile |
| --- | --- | --- |
| `RenameTab` round trip over IPC | 55.0 to 55.4 us | 65 to 87 us |
| Commit to the client's projection | 19.0 to 19.8 us | 25 to 37 us |
| The same rename in process | 0.04 us | 0.08 to 0.12 us |

### Slice 3b: viewers, updates, and input over IPC

- Remote viewers, update pumps with credit, status slots, scrolling, input
  sequencing with staging, the input window and `InputAck`, per-viewer
  input budgets in core, client-numbered scrolls
  in core, and the disconnect order with its core change.
- Contract scenarios for snapshots, scrolling, input, focus, geometry
  control, and revocation.
- Seeded fault injection over the real `huterm-server` and `huterm-client`:
  an in-memory transport with seeded latency, jitter, fragmentation,
  bounded buffers, stalled and trickling readers, connection cuts, and
  decodable damage to deltas, so each run replays from its seed. It takes
  `huterm-ipc-sim`'s broker fault profiles as its specification without
  depending on the crate, which `mise run architecture` forbids. The normal
  suite runs bounded seeds; `mise run faults:ipc` runs many more.

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
  encoded output stays within `OUTBOUND_BUDGET` plus one ordinary frame and
  one effect frame.
- A scroll the server refuses does not let later typing undo a scroll the
  user made after it.
- A scroll during a multi-frame update arrives after that update, and the
  client applies both.
- Under seeded fault injection, after each run reaches quiescence, every
  client converges on its terminals' latest snapshots, resyncs recover from
  damage that digests caught, no connection breaks a lane's order or the
  credit rules, and no input reaches a terminal twice or out of order.

#### Revisions during slice 3b

None of these changes a settled decision. Each is an implementation choice
within the contract or a narrow correction to its text, except the window
close, which this slice leaves as it was (see "What an embedded window
close keeps").

The wire inventory:

- **Viewer requests.** Request kinds 46 to 52 are `SubscribeTerminal`,
  `CloseViewer`, `Snapshot`, `Selection`, `SelectUnits`,
  `UpdatePresentation`, and `SetScrollbackCompression`; the test-only
  `PinTab` keeps 45. `SetScrollbackCompression` is a shared setter, so a
  `Mutation`; the others are `Viewer` requests, with `ReportGeometry`.
  `RequestKind::names_viewer` says which requests name a viewer: they
  travel on the viewer's lane in both directions, so they stay in order
  with its input, and the server handles each as it arrives rather than
  in its request queue. `SubscribeTerminal` names an attachment and
  travels on the control lane. `Snapshot` carries an optional scroll with
  the client's number for it and an optional cell for a link lookup; its
  update arrives before its result. Clients number scrolls from 1, because
  core treats a scroll numbered 0 as already superseded; the server answers
  0 with `InvalidArgument`. `CloseViewer` drops the server's viewer, as
  dropping an in-process one does.
- **Viewer messages.** Message kinds 17 to 20 are `Focus` (client to
  server, on the viewer's lane), `ViewerStopped`, `ViewerRevoked`, and
  `LargeResult`, a result whose response may pass the `Result` cap.
  Selection text is the only one, so `RequestKind::large_result` is set
  for `Selection` alone. `Welcome`'s minor-0 vector changed, because it
  lists the caps of the client's kinds and `Focus` is one.
- **Subscription results.** A subscription's success is
  `Response::Viewer { viewer, status }`. A terminal whose runtime stopped
  on its own answers a success too, `Response::Stopped { status }`, rather
  than the `Stopped` error the text above describes, because a `WireError`
  carries no payload and the final status must travel with the answer.
  Core's `Mux::terminal_status` reads it under the same attachment and
  session check as a subscription. A runtime that panicked publishes no
  final status, so its answer carries the last status it published.
- **The `Result` cap grew.** A viewer's status, about 1.1 MiB at every
  limit, and a link lookup must fit a `Result`, so its cap is about 1.2 MiB
  and a result may span frames; slice 3a's rule that every result fits one
  frame at the smallest `max_frame` no longer holds, though a close
  assessment still does. The retained bound is unchanged, because the
  hierarchy cap is larger still. A link's destination is at most
  `MAX_LINK_DESTINATION_BYTES` (32 KiB), its cells at most `MAX_LINK_CELLS`
  (16,384), and their texts together at most `MAX_LINK_TEXT_BYTES` (32 KiB),
  core's scan limits; a link past them travels as `LinkLookup::ScanLimit`.
- **Responses** 12 to 16 are `Viewer`, `Stopped`, `Snapshot { link }`,
  `Text { text }`, and `Range { range }`.
- **Subscription options** carry the capabilities, initial focus,
  geometry, presentation, and the spawned flag. Host effects arrive with
  slice 4, so a subscription that asks for them answers `Unsupported`.
- **Input cost.** `viewer::input_cost` is what one input counts against
  the input window, on both sides: its text's bytes, a character chord's
  text and layout text, and at least `INPUT_EVENT_COST` (32 bytes), what a
  key or mouse report counts. The floor is about what a staged input
  occupies, so a flood of tiny inputs cannot stage far more memory than
  the window suggests. `viewer::check_input` refuses input past its
  payload limits before the client numbers it, so an oversized paste
  answers `LimitExceeded` locally instead of ending the connection.

The streaming encoder:

- **Frame at a time from the snapshot.** `UpdateEncoder::plan` numbers the
  next update and fixes its baseline, holding only the two `Arc` snapshots
  core already shares. `PlannedUpdate::frames`, called when the message
  starts, builds the style table and each full row's runs without copying a
  cell's text, sizes every row exactly, and encodes the header and style
  table. `UpdateFrames::fill` then encodes one row operation at a time
  straight from the snapshot's rows, so no `TerminalUpdate` value and no
  whole-message buffer exist, and between frames it holds at most the rest
  of one row. A row that encodes to another length than its sizing pass
  computed is an encode error, which ends the connection rather than
  corrupting it. A property test checks that the streamed bytes equal
  `encode_message`'s at any frame size, and a perturbed sizing pass fails
  it.
- **Late encoding.** A status and an input acknowledgement encode when
  their message starts, and a selection result streams its text from its
  own buffer between a head and a tail that `text_result_parts` encodes.

The server:

- **Viewer ports.** The connection holds each viewer as an
  `Arc<dyn ViewerPort>`. It calls the port's non-blocking methods itself,
  polling what changed and admitting ordered messages, so a keystroke
  reaches core without a worker hop. Snapshots, selection text, and
  selection units are `Job::Viewer`s whose replies the native driver
  awaits on its executor; only a subscription, which takes the `Mux` lock,
  runs on a worker. Core's `TerminalViewer` is the production port; the
  test-only `ModelTerminal` is a deterministic one.
- **The viewer's lane opens with its subscription's result**, its first
  message, rather than gating the lane on a result queued on the control
  lane: the order is the same with nothing to gate.
- **Pumps.** A viewer has at most one snapshot in flight. A client's
  snapshot requests go first, then an update owed without credit (the
  first, or a resync's complete snapshot), then a credited update when the
  terminal published something the viewer has not built. A status replaces
  the viewer's unsent status in place, or goes in ahead of the first
  update not yet started; an input acknowledgement does the same.
- **Ordered staging.** Input, focus, and the viewer requests core orders
  with them (geometry, presentation, history edits) share one staging
  queue per viewer, admitted in arrival order, retried every
  `STAGING_RETRY` (16 ms) while anything is staged. Input sequences must be
  consecutive from 1, and staged input past the window is a protocol
  violation. The server acknowledges input once core admitted or refused
  it for good, so input refused for good still leaves the client's window.
  Focus is state, so a focus report staged right behind another replaces
  it: staged focus reports never outnumber the input and requests around
  them, which the window and the in-flight limit bound.
- **Stop and revocation.** Core cannot snapshot a runtime that stopped, so a
  stopped viewer's complete final update re-encodes the last snapshot the
  server planned, then `ViewerStopped` follows. An ended viewer's lane takes
  nothing new but sends everything it already holds, the notice after the
  rest; a viewer the client closed drops only views not yet started. A
  viewer that ends while its subscription's result still waits for room ends
  once that result is on its lane, so the notice never overtakes it, and the
  final status its end consumed goes out first. A snapshot that fails leaves
  core's viewer notified, so the pump keeps owing it rather than waiting for
  a wake core will not send. A connection remembers its last
  `MAX_ENDED_VIEWERS` (256) ended viewers, so a request naming one answers
  `Stopped` or `Revoked` rather than `StaleTarget`; the client keeps the
  same record and answers such a request locally.
- **Disconnect.** The connection drops its viewers, then detaches its
  attachments once no viewer job still holds a viewer, so a job a fault
  harness holds cannot make the detach revoke a viewer it would otherwise
  find dropped. Staged input is discarded.
- **Bounds.** `MAX_CONNECTION_VIEWERS` (256) counts subscriptions in flight.
  The outbound queue's lanes are dynamic, one per viewer. An update or
  selection text counts its frames from their cut until they are written; a
  status or acknowledgement counts whole from its start, when it is encoded.
  Over the budget, while frames are with the writer or a started message
  still has frames to cut, no such message starts, so a connection's encoded
  output stays within `OUTBOUND_BUDGET` plus the messages already started,
  however many viewers it has. One already started still finishes, because
  it holds a stream that messages admitted earlier may need; stopping it
  there could hold every stream while encoded messages sat over the budget.
  When nothing drains, one starts even over the budget, because everything
  retained then waits behind such messages; a status ahead of an encoded
  notice on one lane held a connection that way. A large result counts as in
  flight until its text is written, not only until it is answered, so a
  connection holds one selection's text at a time. The writer-stall deadline
  is armed while anything waits to be written, queued but not yet encoded
  included; it used to count only retained bytes, so a full socket with
  nothing but late-encoded items queued never armed it.
- **Native I/O.** The socket is non-blocking. While the writer is idle, the
  driver writes a batch itself and hands the writer only what the socket
  did not take; the reader and writer wait with `poll` where a blocking
  socket would block. `MSG_DONTWAIT` on a blocking socket was tried first,
  and macOS blocked on it. One task per viewer forwards its wakes, with at
  most one queued at a time.

The client:

- **One stream.** The client keeps sending every message on stream 0,
  whole, in order, as slice 3a did, so each lane's order holds trivially.
  A large paste therefore delays the same connection's other viewers'
  keystrokes by its write time, about a millisecond per MiB locally;
  interleaving client lanes is left to a remote transport that needs it.
- **Viewers.** `ClientSession::subscribe_terminal` takes whether the viewer
  starts visible, so a background tab gets no credit from the start. A
  viewer ends on the client as its `CloseViewer` goes out, since nothing
  after it reaches the server's viewer, and a cancel reaches a request that
  waits in a viewer's queue. A visible viewer keeps the credit window
  full, returning a credit for each credited update it disposes of; a
  hidden one returns none. Input past the
  window waits locally, with focus reports and the requests that name the
  viewer behind it, in issue order. Scrolls are numbered by the session
  and typing carries the number. Per incarnation the session keeps each
  terminal's status baseline and each stopped terminal's final view, and
  `ViewerEvent` reports updates, status with what changed, input
  acknowledgements, stops, revocations, and contract faults.

What an embedded window close keeps:

- Core's detach, retarget, and session close now skip viewers that are
  already dropped, so their admitted input runs before they finalize, and
  an explicit detach of a live viewer still revokes it and discards its
  queued input; cross-session moves still revoke every viewer of the moved
  terminal. A disconnect drops its viewers before it detaches, so a
  remote client's admitted input reaches the PTY.
- The desktop's window close commits first and drops its views after, so
  at the detach its viewer is still live, and input queued at that instant
  is discarded as before. Keeping it needs the desktop to drop the
  window's views before it commits, and to resubscribe them if the commit
  fails; that change touches the close path other open work is editing and
  belongs with [#45][issue-45], which moves the desktop behind the runtime
  interface. Core's test covers the drop-then-close order the change will
  rely on, and the viewers and close smokes stay green.

Fault injection:

- `huterm-server`'s test-only `World` joins real `Connection`s and
  `ClientSession`s through in-memory links on a virtual clock, behind model
  terminals, and runs handed-out work at once or, when it holds work, after
  random delays in a random order. `Tamper` damages deltas in flight so
  they still decode. Every choice draws from one seed, and the viewer
  tables are ordered maps and each world numbers its own viewers, so a run
  replays from its seed.
- Scenarios `clean`, `local-faults`, `remote`, `remote-healthy`,
  `stalled-readers`, `state-corruption`, and `swarm` take the simulation's
  broker profiles. The faulty ones also revoke viewers in their terminals
  and close them from their clients, and the clients subscribe again.
  After the faults, every viewer turns visible, the
  terminals publish past the next update digest, and each run checks that
  every view shows its terminal's latest snapshot, that no client resyncs
  without damage or faults a viewer or the connection, that the server
  ends connections only as stalled, that no input arrives twice or out of
  order, that on a healthy link every input arrives, and that no connection
  retains more than its bound. The normal suite runs three seeds per
  scenario; `mise run faults:ipc -- 1000` ran 1,000 per scenario with about
  32,000 cuts, 42,000 stalls on faulty links and 135,000 from stalled
  readers, 14,000 revocations, 14,000 closes, and 82,000 damaged deltas,
  all caught.
- The world's acceptance cases found one server bug: the stall deadline
  never armed while a full socket left only late-encoded items queued.
  Review found more, each now covered by a test that failed before its
  fix: an ended viewer's lane discarded its notice and queued results;
  selection texts escaped the one-large-result limit; staged focus reports
  had no bound; oversized input ended the connection; a closed viewer
  stayed registered on the client; a cancel missed requests in a viewer's
  queue; a failed snapshot froze its viewer; streamed messages could hold
  every stream while encoded ones sat over the budget; and encoded messages
  over the budget queued behind unstarted streamed ones held the connection
  for good, which a test for a lost final status found.
  The long runs found only the harness's own bugs: unordered maps and a
  process-wide counter that kept seeds from replaying, model viewers whose
  identities collided across terminals, and a tamper that dropped a
  message's first frames when removed mid-stream.

Measurements, release medians on an Apple M3 Max (`mise run bench:ipc`),
each cell the range over two to four runs. "Base" is slice 3a's commit on
the same machine. "Writer threads" is this slice with the drivers handing
every write to their writer thread, as slice 3a did; "direct writes" is
this slice as it ships.

| Measurement | Base | Writer threads | Direct writes |
| --- | --- | --- | --- |
| `RenameTab` round trip | 55.2 us | not measured | 38.1 to 39.0 us |
| Commit to the client's projection | 19.8 us | not measured | 14.8 to 16.6 us |
| Keystroke echo through a PTY over IPC | none | 81 to 107 us | 70 to 73 us |
| The same keystroke echo in process | none | 36 us | 36 us |
| Echo output, in-process viewer to remote client | none | 24 us | 19 to 21 us |
| 100,000-line flood until a viewer shows it, over IPC | none | 147 to 151 ms | 154 to 158 ms |
| The same flood in process | none | 146 to 151 ms | 148 to 150 ms |

A keystroke's echo over IPC still crosses the client's driver, the server's
reader and driver, and the client's reader and driver, besides core's own
hops; replacing the reader threads with an event loop, as "Risks"
anticipates, is the next step if it matters. Direct writes trade about 4
percent of flood throughput for that latency: with writer threads the
flood over IPC took about as long as in process. Writing only small
batches directly did not recover it, which points at the smaller, more
frequent writes rather than inline writing of large batches.
`ci:benchmarks` gates none of these, because no CI runner has measured
them.

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
| A handover looks like a reconnect with renegotiation | Slice 3a same-incarnation renegotiation test; slice 3b status-baseline carry-over test; the simulation's `handover` scenario |
| Bounded memory per connection | Slice 1 stream, cap, and reassembly tests; slice 1 and 2 core invariant tests; slice 3a flood, slow-reader, stall, and undo-slot tests on the injected clock |
| Partial writes and slow or disconnected consumers | Slice 5 byte-split tests; slice 3a and 3b stall and disconnect tests |
| Retry behavior | Slice 3a tests for in-flight runtime queries, viewer requests, mutations, and shared setters; slice 3b input-window test, in which no input is lost or repeated |
| PTY parsing and other viewers continue when a client stalls | Slice 3b stalled-client test that asserts another viewer's updates and PTY output progress |
| `wasm32-unknown-unknown` | CI check of `huterm-protocol`, `huterm-wire` with its vectors, and the sans-IO `huterm-client`; the vectors decode in native tests |
| Architecture boundaries | New `mise run architecture` rules, each shown to fail once against a deliberate violation |
| Peer authentication and socket permissions | Slice 5 tests, including a second-user test in Docker |
| The design's invariants hold under faults, version skew, and remote-like links | `huterm-ipc-sim`: every scenario over 12 seeds in the normal suite, the design-switch tests, and `mise run sim:ipc -- 2000` before changing the design |
| The real server and client hold those invariants under faults | Slice 3b fault injection: every scenario over 3 seeds in the normal suite, and `mise run faults:ipc` over many |

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
- **Major version bumps.** A major bump ends the compatibility window, so
  the GUI update that brings it cannot attach to the running server until
  the server restarts and loses its terminals. Before the first major bump,
  decide whether that release's client also speaks the previous major
  version, or whether a handover that crosses the bump exists by then.
- **Handover state.** The handover feature must carry every value listed in
  "Live handover stays possible". A core change that adds state the
  protocol relies on within an incarnation, such as a new counter, must
  add it to that list.
- **The nightly toolchain.** A second toolchain adds cache and bump work.
  The fuzz job is scheduled and optional, so a broken nightly never blocks a
  pull request.

## Corrections to #44's text

Proposed edits to the issue, for the maintainer to apply:

- "Serialize the envelope's `StreamId`": the wire sends the incarnation once
  per connection in `Welcome` and only the sequence per event; the client
  rebuilds the `StreamId`.
- "Stable IDs scoped to server incarnation": add that `TerminalId` becomes a
  scoped ID, that `RuntimeId` allocation moves into `huterm-protocol`, and
  that a later live handover keeps the incarnation, which changes only when
  the terminals are lost.
- Add the compatibility window: every minor version of the same major, in
  both directions, so a GUI update never requires a server restart.
- "Update the protocol dependency policy deliberately": the policy does not
  change; serialization lives in `huterm-wire`, and the gate gains rules.
- The tests criterion gains property tests and nightly cargo-fuzz targets
  for the codec and update application.
- Dependencies: #22 has landed.

## Open questions

None blocking. The starting values in the bounds table are proposals that
each slice may tune with evidence, recorded in this plan when they change.

[herdr-handoff]: https://github.com/herdrdev/herdr/blob/master/src/server/handoff.rs
[herdr-session-state]: https://herdr.dev/docs/session-state/
[issue-18]: https://github.com/jimeh/huterm/issues/18
[issue-19]: https://github.com/jimeh/huterm/issues/19
[issue-21]: https://github.com/jimeh/huterm/issues/21
[issue-22]: https://github.com/jimeh/huterm/issues/22
[issue-23]: https://github.com/jimeh/huterm/issues/23
[issue-26]: https://github.com/jimeh/huterm/issues/26
[issue-42]: https://github.com/jimeh/huterm/issues/42
[issue-44]: https://github.com/jimeh/huterm/issues/44
[issue-45]: https://github.com/jimeh/huterm/issues/45
[issue-46]: https://github.com/jimeh/huterm/issues/46
[issue-47]: https://github.com/jimeh/huterm/issues/47
[issue-48]: https://github.com/jimeh/huterm/issues/48
[issue-49]: https://github.com/jimeh/huterm/issues/49
[issue-80]: https://github.com/jimeh/huterm/issues/80
[issue-81]: https://github.com/jimeh/huterm/issues/81
[issue-83]: https://github.com/jimeh/huterm/issues/83
[issue-88]: https://github.com/jimeh/huterm/issues/88
[issue-98]: https://github.com/jimeh/huterm/issues/98
[issue-131]: https://github.com/jimeh/huterm/issues/131
[issue-132]: https://github.com/jimeh/huterm/issues/132
[issue-133]: https://github.com/jimeh/huterm/issues/133
[issue-134]: https://github.com/jimeh/huterm/issues/134
