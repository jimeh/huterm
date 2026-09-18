# Terminal metadata and visual bell

Status: approved. The implementation baseline is `main` at `3156f428`, which
contains the tab-bar work merged through
[PR #128](https://github.com/jimeh/huterm/pull/128).

Related roadmap work:

- [#37: Track shell directories and prompt/command lifecycle markers](https://github.com/jimeh/huterm/issues/37)
- [#81: Surface terminal bells, notifications, and progress](https://github.com/jimeh/huterm/issues/81)
- [#137: Add terminal directory and foreground-process metadata to tab labels](https://github.com/jimeh/huterm/issues/137)
- [#138: Present visual bell attention in terminal tabs](https://github.com/jimeh/huterm/issues/138)

## Outcome and delivery boundary

Deliver one medium-sized PR that makes terminal tabs more informative without
pulling in command-history or native-notification infrastructure:

- capture OSC 7 working-directory reports as bounded runtime metadata;
- report a best-effort local foreground-process name from one bounded process
  scan shared by the runtime;
- offer title, process, directory, and combined tab-label modes;
- inherit a usable local reported directory when opening a new tab; and
- present a configurable visual bell without stealing focus.

The existing parent issues are broader than this delivery. The narrower work
is tracked by issues #137 and #138 under #37 and #81. The PR should close those
two delivery issues while leaving #37 and #81 open.

Do not include OSC 133 command records, command navigation, shell-hook
installation, audible bells, native window-attention requests, OSC 9/777
notifications, OSC 9;4 progress, persistence, IPC, or server/remote-client
delivery. Those features require distinct history, policy, permission, and
recipient-routing decisions.

## User-visible behavior

Keep the current title label as the default so existing tabs do not change
appearance after upgrade. Add tab-label configuration under the post-#128
`[tabs]` table:

```toml
[tabs]
label = "title" # title, process, directory, or process_and_directory

[terminal]
new_tab_directory = "inherit" # inherit or default

[terminal.bell]
visual = true
```

The final spelling should follow the merged #128 configuration conventions,
but preserve these semantics:

| Setting | Behavior |
| --- | --- |
| `tabs.label = "title"` | Current application title, then launched-program fallback |
| `tabs.label = "process"` | Foreground process, then title fallback |
| `tabs.label = "directory"` | Compact reported-directory label, then title fallback |
| `tabs.label = "process_and_directory"` | Both values when present; degrade to either value, then title |
| `terminal.bell.visual = true` | Flash the active terminal and mark an inactive tab until viewed |
| `terminal.new_tab_directory = "inherit"` | Use the active terminal's usable local reported directory; otherwise retain the platform default |

Horizontal tabs use one compact line. Vertical tabs may use two lines when the
merged tab component supports it. Truncation and fit-width behavior remain
owned by the tab component. Full-path tooltips are deferred because Huterm has
no existing tooltip component or behavior to extend.

OSC 7 reports from a different host may be displayed, but they are never used
as local launch directories. Missing, malformed, oversized, inaccessible, or
deleted directories fall back to the existing platform default. A fallback
must not block tab creation.

An inactive tab retains an unseen-bell marker until that tab becomes active in
the current view. A bell in the active tab produces a short visual flash but no
persistent marker. Bells never activate a tab, raise a window, or change
keyboard focus. Disabling visual bells on reload clears existing client-side
bell presentation.

## State and ownership

Directory and process information are current runtime metadata. Bell attention
is a transient client presentation:

| Owner | Responsibility |
| --- | --- |
| Pinned Ghostty parser | Recognize OSC 7 framing and expose the current reported URI |
| `huterm-core` engine adapter | Convert the Ghostty callback into a bounded engine-neutral directory update |
| Terminal runtime | Own the latest directory and foreground-process metadata, its revision, and change events |
| Runtime-host process-metadata scheduler | While requested, capture live clients from Mux and invoke at most one core batch scan per interval |
| `huterm-protocol` | Define dependency-neutral metadata values and full-replacement change events |
| GPUI desktop | Resolve configured labels, capture new-tab inheritance at command invocation, and own per-view bell presentation |

Do not put metadata in terminal cell snapshots. Cell damage, metadata changes,
and transient bells have different invalidation and replay rules. Keep the
existing title event intact; the tab-label resolver combines the title cache
with the new metadata rather than forcing an unrelated title migration into
this PR.

Expose a full-replacement metadata event with a monotonically increasing
terminal-local revision. An existing client can discard stale events without
reconstructing partial updates. Fresh runtimes start with empty metadata; #22
owns initial-state resynchronization for later simultaneous or reconnecting
clients. Do not add a one-off metadata query that #22 would replace. The types
must remain serializable in shape and must not expose Ghostty, GPUI, PTY, or
operating-system process handles.

Ghostty's pwd callback normalizes three input families into one callback. Core
must distinguish them rather than assuming every callback is OSC 7:

- a `file://` URI carries a host and path and may be classified for local use;
- a bare absolute path from OSC 9 or OSC 1337 is display-only because it has no
  host evidence; and
- an empty value clears directory metadata, emitting a replacement event only
  when the current value was nonempty.

Directory metadata keeps the reported host and decoded path distinct and
records whether core considers the location usable on the server host. Only
core performs that classification. Treat an empty host, `localhost`, and an
ASCII case-insensitive exact match for the server hostname as local after
removing one trailing dot. Do not equate short names, `.local` names, or other
suffix variants. Obtain the hostname through Nix's safe `hostname` feature;
update the pinned feature set and run the license check. Reject non-file
schemes, credentials, ports, invalid percent encoding, embedded NULs, relative
bare paths, and values beyond the fixed metadata limit. Preserve a remote host
for display without manufacturing a local `PathBuf` from it. A getter failure,
including invalid UTF-8, rejects or clears that update without failing the
terminal.

Foreground-process protocol metadata contains only a bounded display name.
Keep PID, process-group, start-time, and other change-detection identity inside
core. Read only the operating-system command name, not full arguments or
environment. Prefer the foreground process-group leader when it is live;
otherwise choose the lowest-PID live member deterministically. Strip the path
and a leading login-shell dash before classification and display. When the
foreground group contains only the root shell, publish no foreground
application so the UI falls back predictably. Carry the sampled foreground
group alongside the core-private result and discard it in the runtime if the
live group changed before delivery. Missing or incomplete process evidence
clears the best-effort process label; it does not fail the terminal.

## Process sampler lifecycle

Extend the existing bounded process-table code in `huterm-core::jobs` rather
than adding another `ps` parser. Expose one core batch operation and let the
host that owns Mux schedule it. In embedded mode, the global `Desktop` runtime
owns exactly one scheduler task; a future server host owns its equivalent.
Do not start a self-referential worker from inside `Mux` or retain strong
`RuntimeClient` handles between ticks.

- remain idle under the default title and directory-only label modes;
- while the runtime host has process-label interest, briefly lock Mux to clone
  the current live clients, release the lock, and scan their job contexts
  together no more than once per second;
- stop scheduling when configuration no longer requests process labels and skip
  the process-table scan when no runtime child is live;
- keep the scan off the terminal parser, Mux mutex, and GPUI threads;
- retain the existing one-second subprocess deadline and four-MiB output cap;
- send only changed foreground-process values through runtime control queues;
- request all runtime job contexts first, then collect them under one shared
  deadline rather than waiting up to one second per terminal;
- avoid holding the Mux mutex while requesting runtime job contexts or waiting
  for `ps`;
- stop promptly during Mux/application teardown and never extend PTY shutdown;
- coalesce or skip a tick when the prior scan is still running.

One-second freshness while requested is sufficient for a tab label and avoids
turning process inspection into a hot path. Keep the interval fixed in this
delivery. Put the table reader and tick source behind internal injectable seams
so tests can count scans, control progress, and model failure without real
`ps` timing. Measure scan count and elapsed behavior before considering
configuration.

Add a narrow Mux accessor that clones the current live `RuntimeClient` values
while its caller holds the structural lock. It must not expose terminal storage
or let the scheduler retain those clients after the current tick.

## Implementation sequence

### 1. Establish the post-#128 baseline and delivery issues

PR #128 settled the tab component and `[tabs]` configuration shape. The
implementation branch starts from `3156f428`; `TabView::label` supplies both
rendering and fit-width measurement, and `TabParts::status` supplies the status
icon seam. Issues #137 and #138 use this plan as their shared contract. Do not
change #37 or #81 acceptance criteria beyond linking the narrower work.

Run the existing config/schema checks and relevant tab smoke once to establish
the baseline. Record the exact base revision because UI evidence from the
pre-#128 tab implementation is not transferable.

### 2. Add the bounded runtime metadata contract

Add dependency-neutral directory and foreground-process values, a combined
current metadata record, and a full-replacement change event. Keep revision
assignment in the terminal runtime so directory and process updates share one
ordering domain.

Register Ghostty's `on_pwd_changed` callback. Copy the borrowed value only after
enforcing Huterm's metadata bound, normalize URI, bare-path, and empty forms in
core, and emit a directory update after the current write batch. Getter and
decode failures are nonfatal. Promise end-of-batch ordering with title,
invalidation, bell, and exit events; the current title adapter does not retain
within-batch callback positions. A metadata-only input must produce an event
even when no row is dirty and no snapshot is requested.

Evidence: focused engine/runtime tests for complete replacement, revision
ordering, duplicate suppression, post-exit behavior, malformed and oversized
reports, invalid UTF-8, BEL/ST termination, byte-at-a-time and every meaningful
chunk split, empty clears, bare paths, local and remote hosts, trailing-dot and
case hostname handling, percent encoding, and directory updates with no cell
damage. Run `mise run architecture` for the protocol boundary.

### 3. Add core process sampling and one host scheduler

Refactor the existing job scanner so close assessment and metadata sampling
share the bounded process-table acquisition and parsing code but retain
separate classification results. Let the core batch operation publish only
changed process names. Add one scheduler task to the embedded runtime host,
governed by the loaded label configuration and the existing termination gate.
Capture clients under the Mux mutex, then perform every request, wait,
process-table read, classification, and runtime update after releasing it. The
task must not retain clients beyond its current tick.

Do not reuse close-consent `JobState` as the metadata API. Close assessment
tracks all relevant foreground and background groups with stable evidence;
the label needs only the current best-effort foreground command. Sharing the
scanner avoids duplicated OS work without conflating those contracts.

Evidence: deterministic classification fixtures for idle shells, foreground
commands and pipelines, `exec` within a stable group, child churn, process
exit, zombies, leaderless groups, unavailable evidence, normalized login-shell
paths, stale-group rejection, and Linux's 15-byte `comm` limit. Add lifecycle
tests proving the default title mode performs no scans, multiple interested
terminals share one scan, one overall job-context deadline applies, a blocked
or failed scan cannot hold Mux or runtime shutdown, and overlapping ticks do
not spawn concurrent scans.

### 4. Integrate labels and new-tab directory inheritance

Add configuration definitions, validation, defaults, schema output, reload,
and user documentation. Build one tab-label resolver from custom override,
title, launched program, current directory, and foreground process. A custom
tab name always wins in every label mode. Use the resolved label for tab
rendering and fit-width measurement. Palette and cross-window rename
propagation remain owned by #98; do not broaden this PR into the runtime
hierarchy projection.

Resolve an inherited directory synchronously from the active terminal's cached
metadata when the New Tab command begins. Pass that immutable choice into the
existing background spawn operation. On that worker, use metadata plus a safe
execute/search permission check to verify that the path still names a directory
before acquiring the Mux mutex.
Replace it with the existing platform default before calling `open_tab` when
the check fails. Never probe or retry a directory while holding the Mux mutex;
a stale network mount may block. A path can still disappear after the preflight;
that ordinary spawn race returns the existing open error and is not retried
under the lock. New Window behavior remains unchanged.

Evidence: config and schema fixtures; resolver tests for every missing-value
combination, custom-name precedence, and Unicode/truncation boundary; reload
tests; tab creation tests for local, remote, deleted, inaccessible, and
late-changing directories; and a regression proving a later tab switch cannot
change the captured source.

### 5. Add visual bell presentation

Keep `TerminalEvent::Bell` transient. Add a small client-side state machine
with an injected/test clock for active-terminal flash duration and inactive-tab
unseen state. Feed its output into the merged #128 tab status/icon model and
terminal paint invalidation. Define precedence with exited/error status so a
bell never hides a more important terminal state.

Visibility and activation transitions clear unseen state only for the view
that observes the terminal. A future shared viewer receives the same bell event
through #22's broadcast contract but clears presentation independently. Do not
persist, replay, or put bell state in terminal metadata.

Evidence: clock-controlled state tests for active and inactive bells, repeated
bells, activation, hidden windows, configuration disable/reload, exit, and
status precedence. Avoid fixed sleeps. Assert that handling a bell never calls
focus or tab-selection operations.

### 6. Add native acceptance and finish documentation

Extend the existing desktop smoke infrastructure with raw PTY fixtures that
emit OSC 7 and bell sequences and start a uniquely named foreground process.
Observe the production resolved tab label, visual-bell marker, activation
clearing, and inherited child-shell directory. Keep a uniquely named Linux
foreground-process fixture within the platform's 15-byte `comm` limit.
Publish the resolved label in the smoke evidence state; tab counts alone cannot
prove that the production resolver rendered the expected value.

Run focused checks while iterating, then `mise run verify` before handoff. Run
the relevant native macOS and Linux smoke on each platform; Linux evidence does
not prove AppKit presentation and macOS evidence does not prove X11 behavior.
If native automation cannot inspect the brief active-tab flash reliably, cover
its state machine deterministically and manually verify the rendered flash on
both platforms, recording that limitation.

## Acceptance evidence

| Risk or requirement | Required evidence |
| --- | --- |
| OSC 7 metadata is lost without cell damage | Engine and live-runtime metadata-only fixtures |
| A remote URI becomes a local spawn directory | Host-classification tests and native remote-host fixture |
| Process polling runs while unused or multiplies by tabs/windows | Zero-interest and multi-terminal sampler scan-count tests |
| Process scan blocks shutdown | Bounded failure/teardown lifecycle test |
| Custom names are replaced by metadata labels | Resolver precedence tests plus production desktop smoke |
| New Tab inherits from the wrong tab | Invocation-time capture regression test |
| A deleted directory prevents tab creation | Fallback spawn test |
| Bell steals focus or activates a tab | State/dispatch assertions and native inactive-tab smoke |
| Bell attention replays or becomes shared state | Reattachment/recreation tests with a fresh client state |
| Config changes bypass schema/reload behavior | Config, generated-schema, and reload tests |

## Compatibility and rollback

The default label remains `title`, so disabling the new metadata presentation
restores existing tab text. `new_tab_directory = "default"` restores the
current platform launch directory. `terminal.bell.visual = false` disables and
clears visual bell presentation. Removing the process sampler does not affect
PTY ownership, close assessment, terminal parsing, or tab creation because all
metadata consumers have defined fallbacks.

The protocol additions must be additive and non-exhaustive. Do not freeze IPC
encoding in this PR. Persistence and reconnect may later store or transmit
current directory metadata, but they must never treat a process label as a
command to execute or replay old bell events.

## Rejected alternatives

- **Complete all of #37 and #81 together.** OSC 133 history, OS notifications,
  progress, permissions, and rate limiting would turn one reviewable metadata
  improvement into several independent subsystems.
- **Poll processes from each TerminalView.** Multiple windows would duplicate
  scans, disagree about canonical state, and make future non-GPUI clients depend
  on desktop implementation details.
- **Poll processes continuously under the default title mode.** Forking and
  parsing the full process table every second would spend resources on metadata
  no client consumes; explicit interest keeps the core owner without the idle
  cost.
- **Infer directories or processes from terminal titles.** Titles are
  application-controlled display strings and do not establish host, path, or
  process identity.
- **Parse OSC 7 outside Ghostty.** A second byte-stream parser would duplicate
  framing and chunk-boundary logic already owned by the pinned engine.
- **Store bell attention in runtime metadata.** A durable shared flag would
  replay transient attention and let one viewer clear another viewer's state.

## Unresolved questions

None. The merged #128 API supports the proposed `tabs.label`,
`terminal.new_tab_directory`, and `terminal.bell.visual` spelling. Exited or
error status takes precedence over the unseen-bell marker in the single status
slot.
