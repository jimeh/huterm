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

Tests that need a job in its own foreground group use
`test_support::foreground_job` or `orphaned_sleep`, never `set -m` with `fg`:
macOS `/bin/sh` job control left those jobs stuck in about 1 in 3 full local
runs. The test binary reruns itself as `test_support::job_helper`, calls
`setpgid` and `tcsetpgrp` itself, prints `HELD:<pid>;` once it holds the
foreground, and hands the terminal back after one line. Follow it with another
shell command, since a `-c` shell can exec its last command, which makes the
job a session leader. Like `sleep 30`, it survives a hangup, so only a signal
ends it early.

Core tests wait through `test_support::wait_until`, which has a 10-second
deadline and prints the last observation when it fails. A wall-clock bound
times only the operation under test, not runtime spawn or shell startup. Name
scratch directories with `test_support::scratch_dir`, not the clock.

Separately, `Reports` keeps the root shell's own last local OSC 7 report,
attributed when the root's group (its PID) holds the foreground, as
`TerminalMetadata::shell_directory`. Reports from other groups, such as a
remote shell under `ssh`, never change it; an empty report from the root clears
it. New tabs and splits fall back to it when the effective directory is remote
or unusable.

## Shell integration

`huterm-core::shell_integration` owns executable-relative resource discovery and
launch planning; clients pass dependency-neutral settings on `TerminalCommand`.
Keep effective child environment lookup and executable resolution aligned with
portable-pty. Core clears its owned launch variables before applying each plan,
including with integration off, and always exports resolved resource/bin identity.
Only core/engine normalizes OSC 7. The `kitty-shell-cwd://` parser preserves the
literal path without URL parsing or percent decoding, rejects C0/DEL/C1 controls,
and applies the same host-locality decision as `file://`.

Every injected bash, Apple's 3.2 included, starts as `bash --rcfile
<bootstrap> -i`, and the bootstrap emulates login files. Do not return to
`--posix` with `ENV`: Apple bash ignores `ENV` in POSIX mode, POSIX mode leaves
`inherit_errexit` on after `set +o posix`, and it refuses exported functions
whose names are not POSIX names. Bash accepts long options only before `-i`.
Preserve unset versus empty environment values, startup order, and the
admission token's consumption. Each XDG shell gets a separate resource root.
Fish 4.9.3 provides its full marker stream natively. Nushell 0.116.1 provides
native OSC 133; Huterm replaces enabled native OSC 7 with literal reports
because its unencoded `file://` paths lose directories containing URL
delimiters. Preserve its opt-out.

Huterm writes its own shell scripts. Never consult or copy Kitty's, Ghostty's
bash/zsh, or iTerm2's integration scripts, which are GPL. Since 2026-11-04,
permissively licensed references such as VS Code's scripts and bash-preexec
may be read once their license is checked; record them in
`assets/shell-integration/README.md`, and any copied or closely adapted code
needs a file header and a `third-party/` notice. Bash hooks do not read
history or change `HISTCONTROL`. Bash restores `$?`, `$_`, and `PIPESTATUS`
after `PROMPT_COMMAND`, so bash 4.4+ with `promptvars` puts `D;$?` in the
prompt string and needs no first `PROMPT_COMMAND` entry. `PROMPT_COMMAND`,
`PS1`, and `PS0` are often exported, so nested shells inherit Huterm's text:
keep every `PROMPT_COMMAND` entry inside the per-process `case` guard, and make
`PS1`/`PS0` additions expand Huterm's arrays (never exported) through `${...-}`
with no function call, command substitution, or bare `$?`. Keep Huterm's
`PROMPT_COMMAND` text short: bash-preexec 0.7 sanitizes a scalar
`PROMPT_COMMAND` with an extglob loop that takes seconds on a few hundred
characters once its installer's `:` sits between lines, so its pending
installer stays last. Its `case ... in` header has a line of its own because
bash-preexec skips only commands matching a whole line or `;`-part, and `in`
cannot be followed by `;`. Restore a prompt only while it holds the exact value
Huterm set. Never use `local LC_ALL`: restoring an uninstalled locale warns at
every prompt. Bash 3.2 corrupts `\177` in a `$'...'` array element, and its
`DEBUG` trap can fire inside a redirected group, so it writes C to `/dev/tty`.
Any command that runs before a user's first entry changes the `PIPESTATUS` it
sees. Do not count commands with `\#` in `${var@P}` when `PS0` can do it: bash
5.3 lowers that read by one only once a prompt has been shown. Huterm's entries
keep lines or array elements of their own and are rebuilt at every prompt,
because first-prompt installers rewrite `PROMPT_COMMAND`. bash-preexec's first and
last entries stay first and last; an entry after its last one disarms its
next-command detection. A user `DEBUG` trap present after startup runs inside
Huterm's trap, which hides Huterm's own commands from it. Bash 3.2 also marks
execution from that trap, and it reverts trap changes made inside untraced
functions, so install traps at shell scope. Jim accepted 3.2's loss of full
`PIPESTATUS` on 2026-11-04, and standalone 3.2 subshell compounds remain
without C/D markers. Zsh hook functions preserve `$?`, `pipestatus`, and `$_`,
but a `zsh/sched` command does not: it can restore `$?` and `$_` (its return
value and last argument), never `pipestatus`. Keep zsh decoration in the last
`precmd` hook, and use the scheduled command only when hooks follow it.
Changing the precmd array during iteration only affects the next prompt.
Do not decorate zsh prompts from ZLE widgets or call reset-prompt: Debian and
oh-my-zsh replace `zle-line-init`, and a second reset expands `prompt_subst`
twice. A theme that rebuilds `PROMPT` under ZLE redraws it with reset-prompt,
which expands `PROMPT` at once; `zle-line-pre-redraw` runs only in the key
loop, after that expansion, and `zle -F` handlers and `zle-line-finish` refresh
without it. Huterm's `zle` function therefore marks an unmarked `PROMPT` only
for the caller's reset-prompt and restores the caller's value straight after,
which also survives a transient `PROMPT=... function` assignment. It must not
change options around the builtin, because the caller's options govern the
expansion.
Native PTY tests cover the launch boundary. `HUTERM_REQUIRE_SHELLS=1` makes
shell availability mandatory in CI Checks: Apple Bash 3.2 is required on macOS,
where a `bash` of 4.4 or later is optional; Linux requires one (Ubuntu 24.04
ships 5.2), including its emulated login path. Run the bash tests against
another version by putting its `bash` first on `PATH`. Fixture `.zshenv`
disables global compinit, and restoration assertions compare against the same
shell's plain startup to account for system snippets.
Development resource discovery searches up to six executable-directory
ancestors for `assets/shell-integration`, after packaged layouts, to cover
custom Cargo target directories within the checkout.

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
recorded the request, and retried them once. A reader failure or end of file
parses at most one queue of output already read before the runtime acts on it,
so that output reaches history, a command's final D is parsed before the end of
output settles its record, and a flood cannot delay teardown. Unix PTY
readiness waits include cancellation descriptors; signal them and drop the data
receiver before joining workers. The child waiter owns the physical child
without holding a shared lock across `wait`; bounded teardown uses its
cached-status proxy.
Never join it before signalling and closing PTY handles.

Idle work, such as scrollback compression, runs only from the branch where
output, client messages, and controls were all empty, never on the output
path. It runs in bounded batches that check `Wake::is_pending` between steps,
which every queue's sender sets, and it arms a deadline that joins the
foreground probe's in the idle wait. Do not poll the runtime with controls to
watch such work: each control sets the wake flag and ends the batch. Publish
counters through shared atomics instead, as `CompressionCounters` does. The
runtime tests hold a batch mid-pass with `RuntimeControl::CompressionProbe`
to prove queued work runs before the next step.

## Terminal viewers

Each view reaches a terminal through its own `TerminalViewer` registration (see
[the terminal viewers plan](../plans/terminal-viewers.md)). Terminal status
(title, metadata, lifecycle, bell count, numbered failures) is revisioned
state, published only when a value changes, so floods cost constant memory and
a late viewer reads the current state; a gap in failure numbers is reported,
never hidden. Host-effect admission wakes the selected recipient's own viewer.
Admission waits for its locks instead of dropping a write: it used to answer
`Busy` whenever a view's `try_next` held the queue, and Ghostty dropped the
OSC 52 write. It releases the sink state before it takes the terminal budget,
then the process budget, then the queue. Code that holds the queue or a budget
must not take another of these locks, so free `PendingHostEffect` reservations
after releasing the queue, as `try_next` and `clear_queue` do.
The runtime closes every viewer's wake when it stops, through a guard that also
runs on panic, and closes a revoked viewer's wake when it finalizes that viewer.
Host effects have kinds (`HostEffectKind`). Eligibility, permission, and the
permission generation are per kind, so revoking notifications never cancels a
queued clipboard write; `clear_queue` takes a kind filter and still frees the
dropped reservations after releasing the queue. Notifications are admitted from
`drain_effects` after `notifications::normalize` and the terminal's
`NotificationLimiter`; one over the limit is dropped, never deferred. Progress
is status, not an effect: `StatusPublisher::progress` extends a 15-second
expiry on every report and publishes only a changed value. The loop expires
it at the top of every turn, before the status flush, so neither output nor
controls that never pause hold it, and both the idle and the backed-up waits
include its deadline. Exit clears it and refuses later reports; Reset
Terminal clears it through `EngineEffect::Progress(None)`, because Ghostty's C
reset reports no removal. See
[the notifications and progress plan](../plans/terminal-notifications-progress.md).

Clients that show terminals they do not view, such as the workspace rows'
status dots, follow `Mux::subscribe_status` and read `Mux::terminal_statuses`
instead of registering viewers. `Registry::publish_status` signals that feed
only when a change shows in a summary (exit, a new failure, a bell, progress
starting or ending, the foreground program), never for titles, directories,
or command records, and each terminal joins the feed when Mux takes it
(`insert_terminal`), which signals once. Insert every owned terminal through
`insert_terminal`, restore included, or the feed never hears from it.

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

A terminal's scrollback limit is terminal state, not presentation.
`TerminalViewer::set_scrollback` is an ordered buffer edit, so it needs the
`input` capability, follows earlier input, and applies once queued writes drain;
the latest request from any viewer wins. It carries both the row limit and the
optional byte limit, and publishes only when applying it evicted rows from the
primary screen's history, measured before and after rather than inferred from
the limits. A reload that resends a limit, or raises one that evicts nothing,
wakes no hidden viewer. Views resend it after every viewer replacement: the
revoked viewer's queued requests are discarded, and a subscription does not
carry the limit. `TerminalCommand::scrollback` applies before the child spawns.

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

## Command records

`engine/records.rs` turns the OSC 133 events Ghostty reports into the records
`TerminalStatus::commands` publishes: at most 256, oldest first, each with one
tracked anchor (see [the engine guide](terminal-engines.md#the-huterm-ghostty-crate)).
The engine handles each event at the stop that parsed it, so the cursor and
active screen it reads are the marker's own. A primary A replaces the pending
prompt anchor and abandons a running record; other prompt kinds and B change
nothing. C abandons a running record and starts one at the pending anchor, or
at the cursor once RIS has discarded it. D finishes the newest record only
while it runs, so a redraw's repeated D is ignored. Without OSC 133 `aid`,
an outer shell's D finishes a nested shell's command that was still running
when that shell ended, such as `exit` under `ssh`. A and C parsed on the
alternate screen are ignored; D is not, because a crashed full-screen program
can leave it active. A record changes once, when it leaves `Running`. Command
text comes only from the shell (fish sends it), stripped of controls, capped
at 4 KiB, and never interpreted.

The runtime calls `command_update` after every engine operation: output,
resize, clear, reset, and scrollback limits. It drops records whose anchor lost
its value, except the newest and any that left `Running` since the previous
call, so each status revision carries every final state, and it stages a
changed list for the status flush. Status is latest-value: a viewer that wakes
once across two revisions can still miss a state the second one evicted, so a
consumer that must see every completion (#81) needs its own signal. Root exit
alone settles nothing, because the runtime observes the exit before it parses
output already read. A running record is abandoned once the root has exited
and the reader has ended (`PtyEof` or a reader failure), and at teardown, which
parses no more output: a reader failure can stop the loop before it observes
an exit that already happened. A macOS session leader's exit hangs the
terminal up, so the end follows at once; on Linux a descendant that ignores
SIGHUP keeps it open, and the record stays running until that descendant
closes it.

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

`run_terminal` runs the same teardown when its loop panics, then resumes the
panic, so `shutdown` still returns `ThreadPanic` and every wake closes. After a
panic it publishes no status, and signals only the shell, foreground, and
already recorded close groups; it runs no process-table scan.

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

SessionId, WorkspaceId, TabId, PaneId, SplitId, AttachmentId, ViewerId, and
TerminalId carry a RuntimeId as well as a numeric ID. Validate the runtime scope
on every structural target, including move anchors; matching socket names do
not permit transfers between Mux instances, and the hierarchy reducer refuses a
layout whose pane, split, or terminal is another runtime's. The numeric `new`
constructors create unscoped IDs for fixtures/restoration input and are
rejected by structural APIs; captures stay unscoped and a restore scopes them,
terminals included. `RuntimeId::allocate` in `huterm-protocol` hands out every
RuntimeId in the process, so an embedded Mux and a remote connection never
share one. Across processes, a `ServerIncarnation` names one lifetime of a
server's terminals, and a remote `StreamId` carries it. Scope checks prevent
accidental aliasing, not deliberately fabricated IDs.
Every hierarchy state stays encodable because core holds three invariants on
every path, embedded mode included: custom names of at most `MAX_NAME_BYTES`
(256; longer is `InvalidName`, fallback names truncate at a character
boundary), at most `MAX_HIERARCHY_ITEMS` (16,384) sessions, workspaces, tabs,
panes, splits, and attachments together, and layouts at most
`MAX_LAYOUT_DEPTH` (64) nodes deep, which `PaneLayout::new` enforces. A
structural operation checks the item count through `Mux::admit_items` before
it allocates, so a refusal leaves the revision and close tickets alone; a new
operation that adds items must call it too. A restore checks names and items
in `within_limits` before it spawns anything, not in `RuntimeCapture::validate`,
whose looser bounds `huterm-state` shares.
A session or workspace takes its automatic name's number when it is created
and keeps it, never from its membership position. Sessions number across the
runtime; workspaces number within their session, on a counter each
`Session` keeps, so every session's first workspace is "Workspace 1". A
workspace moved in raises its new session's counter to its number.
`Mux::set_automatic_numbering` picks the number: the lowest one no live
record of the kind holds, among the session's workspaces for a workspace
(the default, so a closed record's number is reused), or one above the
highest handed out.
Custom-named records keep their automatic name, so their numbers stay taken and
Clear Name never repeats a live name. The client stores
`[general] session_numbering` in an atomic that every `DesktopRuntime::lock`
applies, so a reload orders with later creations without the UI thread taking
the lock. `None` clears an override; blank custom names are invalid. Names
come from `Desktop.hierarchy`, the client's projection of
core's sequenced hierarchy events. The UI thread takes the Mux lock for
structure only once, at startup before any window exists, to subscribe.
Tab labels look tabs up there by ID and combine them with the live terminal
title. Structural results carry the hierarchy sequence they committed at, and
completions wait for the projection to apply it before acting. Client effects
derive from projection state through `reconcile`, which removes views, orders
tabs, publishes titles, and installs views for tabs a window did not spawn. See
[the hierarchy projection plan](../plans/hierarchy-projection.md).
A tab holds a `PaneLayout`, the n-ary split tree in `huterm-protocol`; its
constructor and transforms validate every invariant, so core, the reducer, and
tests share one implementation (see [the split panes plan](../plans/split-panes.md)).
`split_pane` spawns the terminal before it changes the layout, so a failed spawn
changes no structure and emits nothing. Closing a tab's only pane closes the
tab. Splits and pane closes advance `Mux::revision`; `resize_split`,
`set_pane_zoom`, and `arrange_panes` do not, because none changes which
terminals a close covers and a stale commit makes the desktop show its
confirmation again. Every layout change emits the whole layout in
`PaneLayoutChanged`, with the shared zoom, so removing the zoomed pane clears
the zoom in the same event. Pane IDs are
scoped like tab IDs, and the reducer rejects a layout that reuses another tab's
pane or terminal.
Pane moves (`mux/moves.rs`) never start or stop a terminal: a move changes
which tab holds a pane, never the pane or its terminal. A move across tabs is
one `PanesRearranged` event, never two layout events a drain could split, and
the reducer applies it all or nothing, refusing any change to the set of
pane and terminal pairs its tabs hold. Each move computes its layouts and
checks `next_id` before calling `allocate`, which advances the revision before
it detects exhaustion, so a refused move leaves pending close tickets valid.
Moves advance the revision; swaps, like resizes, do not. A tab whose last pane
moves away is removed, unlike an emptied workspace or session.
`arrange_panes` (`mux/arrange.rs`) rearranges one tab's existing panes into a
client-fitted `Arrangement`, for named layouts, cycling, and balancing. It
refuses a tab whose structure no longer matches the caller's `expected` layout
(`PaneLayout::same_structure`, which ignores weights), and builds the result
through `PaneLayout::arranged`, which refuses an added, dropped, or repeated
pane. A new shape's split identities come from `reserve_ids`, which, unlike
`allocate`, does not advance the revision, because no close ticket names a
split; check `next_id.checked_add(count)` before any change, as moves do. A
change clears the shared zoom in its one `PaneLayoutChanged`; a no-op leaves
it.
`restore_tabs` (`mux/restore.rs`) undoes or redoes a move's `LayoutChange`
only while every touched tab keeps the splits, panes, terminals, and session
the change left. Weights, zoom, names, and positions never block it, and
only tabs in `placed` return to their recorded workspace and position. Undo
reopens a removed tab with its old `TabId`. Every kind shares one counter,
so the restore's check that an ID is at or below `next_id` and not in use
refuses only IDs never allocated. Core-produced records are what make reuse
safe; a client that can forge records over IPC needs a stronger check.
A restore advances the revision when a terminal changes tabs or a tab
opens, is removed, or changes workspace or position, and emits
`PanesRearranged` even for one tab, where the forward swap emitted
`PaneLayoutChanged`.
Session restore (`mux/capture.rs`, unrelated to `restore_tabs`) captures
and recreates structure, never processes. `Mux::capture_runtime` records
sessions that hold a tab or a pinned workspace, and the tabless sessions
the caller's `shown` set names (the client's saved windows), in order,
with unscoped numeric IDs, the allocator watermark, the session creation
ordinal, each session's workspace ordinal, and each workspace's pin and
remembered directory, plus up to three
directory candidates per shown terminal, read from registry status
without waiting on its thread: the local effective directory, the root
shell directory, and the launch directory `TerminalRuntime` keeps,
without repeats. It records no program, argument, environment, title,
process, command record, or exit state. Restore runs in two steps so the
Mux lock covers only insertion: `SpawnedTerminals::spawn` starts every
pane's terminal with `spawn_in` outside the lock, checking its
cancellation callback before each spawn and stopping and joining what it
started; then `Mux::restore` refuses unless the Mux is fresh (no
session, workspace, or terminal, `next_id` and both ordinals still 0,
since an emptied Mux may have handed out IDs a restore would alias), the
capture's socket name is the Mux's, and the spawned set came from this
`RuntimeId` for exactly this capture, and validates all or nothing
through `RuntimeCapture::validate`, which `huterm-state` shares. Every
refusal stops and joins the spawned runtimes, and dropping an unused
`SpawnedTerminals` does the same. A restore sets `next_id` to the
watermark, so later allocations of every kind land above restored IDs,
resumes the session ordinal from the highest automatic session name it
restored, custom-named records included, rather than from the saved
ordinal (so increasing numbering stops counting sessions closed before
Quit across restarts), keeps each session's saved workspace ordinal,
raised to the highest automatic name among its restored workspaces (a
file from before per-session counters reads as zero), scopes saved IDs
into its `RuntimeId`, takes pane fallback names from the commands it
launched (captures blank them, since they are process labels), and emits
the creation events so existing subscribers follow. A pane whose terminal failed
leaves its layout through `without_pane`, a tab left with no pane is
removed, and the report names both and every session left without a tab.
Tear-off orphan cleanup asks `Mux::session_attached` under the lock.
Attachments are part of the hierarchy stream: `attach_session`,
`retarget_attachment` (only when the session changes), and `detach_session`
emit `AttachmentCreated`, `AttachmentRetargeted`, and `AttachmentDetached`
under the lock that makes the change, so restore's attachments emit too.
`close_session` emits only `SessionClosed`, which drops the session's
attachments from a projection, and `subscribe_hierarchy` snapshots the
attachment map. The reducer refuses a foreign or repeated attachment, a
retarget or detach of an unknown one, and one naming an unknown session.
Any session's attachment change makes `windows_to_visit` visit every window.
`move_session` reorders the session list and emits `SessionMoved`, which
touches every window's session list through the names summary. It leaves
the revision alone, since no close covers a different set of terminals.
Moves retain empty workspaces and sessions and never change terminal lifetime.
Desktop windows retain their attachment identity for assessed close. Orphaned
spawn cleanup must preserve resources adopted by another attachment or moved
elsewhere. Roll back newly created sessions on initial workspace/tab failure.

## The wire contract

`huterm-wire` encodes and decodes protocol types for the runtime IPC (see
[the plan](../plans/runtime-ipc.md)). It depends only on `huterm-protocol`,
builds for `wasm32-unknown-unknown` with its vectors (`mise run check:wasm`),
and holds no sockets or threads. `mise run architecture` checks the
dependency rule.

- Encoders destructure each protocol struct exhaustively, so a new field
  fails to compile until its wire form is decided. `HierarchyEvent` is
  `non_exhaustive`, so a new variant compiles but encodes as `Unsupported`:
  give it the next tag, an encoding, a decoder arm, and a golden vector.
- Decoders check every length and count against a field maximum before they
  allocate, count hierarchy items, refuse layouts past `MAX_LAYOUT_DEPTH`
  before recursing, and build values through the protocol's validating
  constructors.
- A minor version only appends record fields, adds variants and message
  kinds, and adds codes. Senders always write appended fields and older
  decoders skip them, so an appended field must be safe to ignore and must
  fit the older version's cap; the cap formulas reserve headroom per record
  and per hierarchy item for this. A newer variant that a server can send
  defines its downgrade: `Skipped`, one older event, or a projected resync.
  A change an older peer cannot do without is a major version bump.
- Golden vectors in `vectors.rs` pin each released minor version's bytes.
  Never edit an existing vector's bytes; a new minor version adds vectors.
  Minor version 0 is unreleased until a server ships (#45), so slices may
  still change its records until then.
- `test_minor.rs`, behind the `test-minor` feature, is `MINOR + 1` and
  exercises the downgrade rules; shipping code never enables it.
- Terminal updates (`update.rs`) diff by `Arc<TerminalRow>` identity on the
  server only; the wire carries baseline row positions. A new `CellStyle`
  field needs a flag bit, a minor version, and a projection in
  `UpdateRow::flags` for older peers, because a decoder rejects unknown
  bits. `view_digest` covers exactly the negotiated minor version's fields.
- Core keeps every state encodable on every path, embedded mode included:
  `GridSize::clamped` caps grids at `MAX_GRID_CELLS`, row extraction cuts
  cell text at `MAX_CELL_TEXT_BYTES`, and `StatusPublisher` cuts status text
  at `MAX_STATUS_TEXT_BYTES`. A new path that builds a grid, a cell, or
  status text must apply the same cap.
- `StatusBaseline` in `huterm-protocol` derives exits, bells, and failures
  from status for `TerminalViewer::poll` and remote clients alike.
- `fuzz/` holds cargo-fuzz targets outside the workspace;
  `mise run fuzz:wire` runs them (see
  [the dependencies guide](dependencies.md#fuzzing-toolchain)). Turn a crash
  into a regression test in the normal suite.

## Attachments, close consent, and exit

Session AttachmentId is distinct from a terminal viewer (`ViewerId`); one
attachment holds a viewer per displayed terminal, and detach, retarget, session
close, and cross-session moves revoke its viewers. Detach
and retarget preserve zero-view sessions; an assessed last-window close deletes
its session. `CloseRequest::EmptiedWindow` closes a window whose last view went
away without stopping a terminal, resolved under the Mux lock in this order: an
invalidated attachment fails with `UnknownAttachment`; a workspace that still
holds tabs in the attachment's session fails with `ViewNotEmpty`, and a
pinned one with `PinnedWorkspace`, so a pinned workspace outlives its last
tab even as its session's only one; any other workspace in that session,
with tabs or empty, gives LeaveWorkspace, which keeps the attachment and
closes the emptied workspace unless it is pinned; otherwise another
attachment on the session gives Detach, and else Session deletes the empty
session. No resolution needs consent, and the revision check turns a tab
opened, a workspace created or closed, or a pin changed after assessment
into `StaleClose`, so `set_workspace_pinned` advances the revision. Every
path that removes a workspace's last tab (`close_tab`, `move_tab`,
`remove_tab_record`) calls `remember_directory` before the terminal stops,
so New Tab in the empty workspace can start there.
Desktop close uses prepare/check/commit and request generations.

A runtime can stop while Mux still owns its tab: its owner panicked or a fatal
reader or writer failure ended it, and its own teardown signalled and reaped the
child. The viewer `Registry` closes whenever the runtime thread ends, including
by unwinding, so `Registry::is_closed` means "stopped on its own" for a terminal
Mux owns. `CloseTicket::check_jobs` assesses such a terminal as `Idle` without
asking it, reading the stop at check time so a recheck sees a later stop too.
`close_tab` and `close_terminals`, and so `shutdown`, read the stop before they
signal any runtime and then drop that runtime's join error, which its viewers
already saw as `Stopped`; `ShutdownTimedOut` still reports, because a child may
be alive, and a runtime that panics during the close still reports
`ThreadPanic`. Mux keeps the tab and emits nothing when a runtime stops: only
an explicit close removes it. `RuntimeClient::close` marks the registry
before it signals the runtime, so `TerminalViewer::close_requested` tells a
viewer that a `Stopped` it sees came from a requested close or teardown, not
from a runtime that stopped on its own. A close after such a stop leaves the
mark unset, so its viewers still report the stop Mux's close drops.
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
A write that finds the terminal hung up (macOS `EIO`; Linux `POLLHUP` once its
buffer fills) ends input the same way without failing the runtime, because the
shell's exit can hang the terminal up before `try_wait` reports it. The writer
closes input before reporting the hangup. Every write passes through
`Owner::queue_write`, which drops it once input closes; do not add per-path
exit checks. A process that reopens `/dev/tty` after a hangup gets neither input
nor output, as the reader also stops at end-of-file. A macOS session leader's
terminal hangs up only at its exit; tests revoke the slave to hang it up
earlier.
