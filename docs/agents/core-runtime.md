# Core runtime and terminal lifecycle

Rules for `huterm-core` and `huterm-procinfo`: PTY I/O, process inspection,
runtime workers, shutdown, the Mux, and terminal lifecycle. The boundaries in
[AGENTS.md](../../AGENTS.md) still apply.

## Process information

`huterm-procinfo` reports operating-system process facts: Linux through
`/proc` with the standard library, macOS through `proc_pidinfo`,
`proc_listpids`, and `sysctl`. Keep its unsafe calls in `macos.rs`. Other
users' processes, such as `sudo` jobs, expose only short BSD info there,
without a TTY or start time; read their start time from the leading `timeval`
of `sysctl(KERN_PROC_PID)`, both before and after the short-info read, so a
reused PID cannot mix two processes' facts. `proc_listpids` returns 0 for both
an empty list and an error, so clear `errno` before each call. The crate
depends on no Huterm crate (`mise run architecture` checks this). Runtimes
probe their foreground only after Enter or job-control input, output after
silence, or a title change, plus a one-second poll while a job, or an empty
group left by an exited job, holds the foreground. Idle shells arm no
deadline, and the output path never calls `tcgetpgrp`. Never probe at spawn: a
child that has not exec'd yet reads as Huterm. An OSC 7 directory or OSC 0/2
title report applies only while the foreground group that was active when it
arrived keeps the foreground; otherwise publish the probed foreground
directory, falling back to the root's. Read the group for a title only when
its text changes, its recorded report no longer applies, job-control input
arrived since the last probe, or a repeated title's attribution is 250 ms old.
The first probe publishes the shell's directory about 50 ms after its first
output, which changes the status. A test that asserts no status change must
first wait for that directory, and must not fork per line, or it passes only
while it outruns the probe.

## PTY I/O and descriptor limits

- On Unix, configure the PTY master as nonblocking before cloning reader and
  writer handles; the clones share its open-file-description flags.
- After a nonblocking PTY read returns `WouldBlock`, wait for readability
  instead of sleeping before every retry. Wait through `nix::poll` on every
  Unix platform. Descriptors at or above `FD_SETSIZE` (1024) must remain
  usable, so never wait through `select(2)`, including `filedescriptor::poll`,
  whose macOS path uses it. The old "broken macOS `poll(2)`" rule came only
  from that crate's comment. A macOS 27 probe at descriptors 1500 and 1700
  saw correct PTY master readability, write backpressure, drain, hangup, and
  socket-pair cancellation, and Ghostty's read thread polls its PTY the same
  way.
- The GPUI desktop bootstrap (`run_with_startup`) calls
  `huterm_core::raise_open_file_limit()` before creating its application. Any
  other binary that hosts terminals, such as the planned local server or a
  smoke outside that bootstrap, must call it first at startup. Following
  Ghostty, it raises the soft `RLIMIT_NOFILE` and records the original, which
  `pty::spawn` passes to the vendored portable-pty `nofile_limit` setter so
  children start with the limit Huterm inherited. Without the raise, a
  launchd soft limit of 256 stops terminal creation after about 28 terminals,
  or fewer once the app's own descriptors count. Tests that lower the
  limit, exhaust the descriptor table, or call `raise_open_file_limit()` live
  in `crates/huterm-core/tests/descriptor_limits.rs`, their own process. Core
  unit tests may only raise the soft limit and must release filler
  descriptors when they finish.
- Concurrent `openpty` calls failed on macOS 27, so `pty::spawn` serializes PTY
  creation.

## Runtime queues and workers

Runtime data and priority controls share a coalesced wake; keep control drains
bounded so output cannot starve. Client input, resize, and presentation
messages have their own bounded queue, taken ahead of PTY output and
alternating with it when both wait, so an output flood cannot refuse input.
Writer dequeue wakes the runtime only after the runtime has spilled writes,
recorded the request, and retried them once. Unix PTY readiness waits include
cancellation descriptors; signal them and drop the data receiver before
joining workers. The child waiter owns the physical child without holding a
shared lock across `wait`; bounded teardown uses its cached-status proxy.
Never join it before signalling and closing PTY handles.

## Terminal viewers

Each view reaches a terminal through its own `TerminalViewer` registration (see
[the terminal viewers plan](../plans/terminal-viewers.md)). Terminal status
(title, metadata, lifecycle, bell count, numbered failures) is revisioned
state, published only when a value changes, so floods cost constant memory and
a late viewer reads the current state; a gap in failure numbers is reported,
never hidden. Host-effect admission wakes the selected recipient's own viewer.
The runtime closes every viewer's wake when it stops, through a guard that also
runs on panic, and closes a revoked viewer's wake when it finalizes that viewer.

Reconcile only through `Owner::reconcile`, which runs when the registry changed
and waits while writes are backed up: freed slots would let subscribe-and-drop
churn queue focus reports without bound. Call it at the top of each turn and
again in `handle_client_message`, so every request has its viewer's entry and
follows any finalization that preceded it; a mouse press with no entry would
take a gesture that finalization could not release. Never restart the turn for
a pending change: that churn then starves requests and output. Requests are
dequeued only once writes drain, and the idle wait rechecks for a pending
change; the backlog consumed the notification. `RuntimeMessage::permitted` is
the one dequeue check for ordered requests, and `handle_client_message` settles
each once. Scroll numbers live on the viewer's slot, not its arbitration entry:
scrolls are controls that can precede or outlive the entry.

Rearm a viewer's invalidation on the runtime owner thread when constructing
that viewer's snapshot, not when draining its notification, so hidden or
frame-blocked viewers do not wake for every output chunk; one viewer's snapshot
never rearms another. Every snapshot-affecting change, including viewport moves
and controller-driven resizes and presentations, goes through the one
publication function, because viewers share a snapshot cache keyed by its
revision. Viewer errors (`NotPermitted`, `Revoked`) answer only the requesting
viewer and never pass through the runtime's fatal snapshot path. Capabilities
are checked by the handle and again at dequeue.

The controlling viewer, the one that most recently gained focus or typed (shown
or hidden, as tmux's `latest`), decides the canonical size and the applied
presentation; a single viewer always controls. Typing returns the shared
viewport to live on the runtime, skipped when the same viewer has a later
scroll applied; the view only drops its own pending scroll intent, unless its
queue refused the input, as in an exited tab; a viewer's scroll sent before
such typing never moves the viewport once the typing has run. Accepted mouse
gestures always end: releases are never discarded for geometry, a finalized
gesture owner gets synthetic releases at its own last position, turning
tracking off stops the held buttons from blocking other viewers but keeps their
releases the owner's until another viewer's press is admitted or the owner
presses that button again, and no report takes a gesture while tracking is off.
After root exit, typing still moves control and returns the viewport to live,
and replies from a resize are dropped with the input.

## Snapshots and link lookup

Construct non-Send native handles on their owner thread before spawning the PTY;
only publish startup after workers are ready. Scroll-and-snapshot share one
ordered control operation. Ordinary snapshots must not reset the viewport.
Complete snapshots share immutable `Arc<TerminalRow>` values; never clear engine
damage before owned cache state is coherent. Content generation excludes scrolling.

Terminal link lookup runs only on the terminal owner thread, paired with its
snapshot. Keep the scan limits (32 KiB text/destination, 128 rows, 16,384 cells,
2 MiB explicit-link comparison work) and preserve viewport/damage state. The
Ghostty native OSC 8 parser uses a fixed 2,048-byte capture: parameters plus URI
may occupy 2,046 bytes; larger sequences are discarded, not truncated.

## Shutdown and reaping

Reap the child again after closing the master and joining I/O workers. The last
master descriptor can deliver the hangup that exits the shell; reaping only
before descriptor closure can leave a zombie. Keep this final wait bounded and
return a cleanup error if the child remains alive. This was exposed on macOS
when the execution sandbox rejected process-group signals with EPERM.

After bounded PTY cleanup times out, transfer the child handle to a deferred
reaper until wait succeeds; preserve ShutdownTimedOut for the caller. If the
reaper thread cannot be created, emergency synchronous reaping can exceed the
normal shutdown bound. Deferred cleanup cannot outlive OS application termination.

## Mux, IDs, and hierarchy

SessionId, WorkspaceId, and TabId carry a RuntimeId as well as a numeric ID.
Validate the runtime scope on every structural target, including move anchors;
matching socket names do not permit transfers between Mux instances. The numeric
`new` constructors create unscoped IDs for fixtures/restoration input and are
rejected by structural APIs. RuntimeId is unique only within this process; future
IPC needs incarnation identity across processes/restarts. Scope checks prevent
accidental aliasing, not deliberately fabricated IDs.
Keep session/workspace automatic names tied to immutable per-kind creation
ordinals, not membership positions. `None` clears an override; blank custom names
are invalid. Names come from `Desktop.hierarchy`, the client's projection of
core's sequenced hierarchy events. The UI thread takes the Mux lock for
structure only once, at startup before any window exists, to subscribe.
Tab labels look tabs up there by ID and combine them with the live terminal
title. Structural results carry the hierarchy sequence they committed at, and
completions wait for the projection to apply it before acting. Client effects
derive from projection state through `reconcile`, which removes views, orders
tabs, and publishes titles but never installs views. See
[the hierarchy projection plan](../plans/hierarchy-projection.md).
Moves retain empty parents and never change terminal lifetime. Desktop windows
retain their attachment identity for assessed close. Orphaned spawn cleanup
must preserve resources adopted by another attachment or moved elsewhere. Roll
back newly created sessions on initial workspace/tab failure.

## Attachments, close consent, and exit

Session AttachmentId is distinct from a terminal viewer (`ViewerId`); one
attachment holds a viewer per displayed terminal, and detach, retarget, session
close, and cross-session moves revoke its viewers. Detach
and retarget preserve zero-view sessions; an assessed last-window close deletes
its session. Desktop close uses prepare/check/commit and request generations.
Run process-table scans off both Mux and terminal-parser threads; carry fresh
assessed background process groups into teardown. Match a terminal's processes
by TTY device number. On macOS, take TTY membership from the kernel's TTY
filter, because other users' processes do not report their terminal.
Root-shell exit completes the terminal, matching Ghostty, iTerm2, WezTerm, and
Alacritty. Reap through portable-pty immediately, even when history is retained;
do not require an empty OS session or warn about post-exit survivors. Clear
historical cleanup groups after exit, and never signal them when closing
completed history. JobLifecycle lets queued live assessments observe exit or
retirement without looking up old process IDs. Live-shell job warnings remain.
Do not restore post-exit ps/getsid scans: concurrent ps children and unrelated
process churn made ordinary macOS exits require confirmation or retain zombies.
Use explicit FIFO release in the surviving-holder test, never a timed sleep.

Close consent follows process groups with stable leader creation identity,
including exec and worker churn; leaderless groups need an original surviving
member. New groups or unknown-state widening require reassessment. Use fresh
members for cleanup. Native cancellation tests must send input and observe a
unique shell ACK after cancel and retry; an exited terminal retains snapshots.

Core drops queued input and terminal replies after root exit, stops the writer,
and keeps final-output parsing, snapshot/selection requests, and emulator resize.
A late writer failure after observed exit must not destroy retained history.
