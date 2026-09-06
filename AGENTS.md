# Huterm agent guide

Huterm is a Rust terminal emulator whose runtime owns PTYs, emulator state,
sessions, workspaces, tabs, and panes. GPUI is one client. The runtime boundary
must remain usable by a future local server and text client.

Read [README.md](README.md) for product scope and
[the initial desktop plan](docs/plans/initial-desktop-poc.md) for the current
architecture and acceptance criteria.

Read [CONTEXT.md](CONTEXT.md) for canonical terminology and
[the workspace plan](docs/plans/workspaces-windows-tabs.md) for the agreed
direction and follow-up milestones. Core owns a logical socket scope with
ordered sessions, workspaces, tabs, and terminal runtimes. Each desktop window
creates a private session and workspace; shared attachments remain deferred.
Keep view destruction and detachment separate from explicit close.

## Boundaries that must hold

- `huterm-protocol` owns dependency-neutral IDs, input, lifecycle events, and
  immutable snapshots. Its public types must not expose GPUI, Alacritty, or
  `portable-pty` types. `mise run architecture` enforces its empty dependency
  set.
- `huterm-core` alone owns PTYs and canonical Alacritty state. One runtime
  thread mutates each terminal. Blocking PTY reads, ordered writes, and child
  waits stay off the GPUI thread.
- `huterm-gpui` owns macOS/Linux window state, rendering, key translation,
  focus, and each client's viewport offset. Scrolling must not mutate shared
  emulator state.
- Use upstream `alacritty_terminal`; do not use its PTY event loop. Do not copy
  or depend on GPL-covered Zed application or terminal-view code.
- Treat child exit, client detachment, and explicit terminal close as distinct
  lifecycle events. Any shutdown change must prove that live children and
  blocked I/O workers terminate.
- On Unix, configure the PTY master as nonblocking before cloning reader and
  writer handles; the clones share its open-file-description flags.
- After a nonblocking PTY read returns `WouldBlock`, wait for readability
  instead of sleeping before every retry. Use `filedescriptor` for this wait:
  its macOS implementation avoids the platform's unreliable PTY `poll(2)` by
  using `select(2)`.

## Commands

Run `mise tasks` to discover the full task set.

- `mise run doctor` checks host prerequisites; `mise run dev` starts the native
  macOS or Linux client.
- `mise run check` runs the fast format, Clippy compilation, docs, and
  architecture gate; `mise run typecheck` remains available independently.
- `mise run test` runs unit and PTY integration tests.
- `mise run verify` matches CI and adds dependency-license and workflow checks.
- `mise run bench:renderer` drives the release renderer under Xvfb and reports
  opt-in CPU preparation and paint-encoding timings.
- `mise run bench:scroll` drives production scroll inputs against 10,000 rows
  and enforces snapshot elapsed-time, wakeup, offset, and bounded-queue budgets.
  Linux runs it under Xvfb; macOS runs it natively and also enforces paint
  elapsed time, reuse, and input latency when the host delivers enough frames.
- `mise run package:macos` builds and verifies the Apple Silicon `Huterm.app`.
- `mise run format` writes Rust formatting and refreshes action pins.

Repository Rust formatting is defined by `rustfmt.toml`; it must not depend on
or require changes to `~/.rustfmt.toml`.
Keep the Rust version, minimal profile, and `clippy`/`rustfmt` components in
`mise.toml` aligned with `rust-toolchain.toml`; CI installs only the named Mise
tools for each job.

Keep `verify:toolchain` as a serial preflight before `verify:parallel`. Mise's
CI cache can restore its Rust install symlink without the corresponding rustup
toolchain, and parallel Cargo invocations then race while materializing it.

Use focused Cargo tests while iterating. Run `mise run verify` before a broad
handoff. GitHub Actions is the source of truth for macOS arm64 compilation and
runs the same checks plus an Xvfb smoke on Linux x86_64. Linux development
requires the XKB packages documented in
[the development guide](docs/agents/development.md).

The pre-commit hook runs staged-path formatting and Markdown checks, then
triggers whole-workspace Clippy or workflow checks only for relevant staged
inputs. Install it with `mise run setup`. Keep its representative warm path
below 10 seconds; CI remains authoritative because hooks can be bypassed.

## Dependency changes

The repository uses a three-day release-age policy for Mise tools, Cargo
updates, and GitHub Actions, and commits `Cargo.lock` and `mise.lock`. Pin
direct runtime dependencies deliberately. Use `mise run actions:update` to
refresh action pins and `mise run tools:update` to refresh project tools
without accepting releases inside the cooldown window.
Keep the cooldown values in `mise.toml`, `.pinact.yaml`,
`.github/dependabot.yml`, `.github/workflows/ci.yml`, and `zizmor.yml` aligned
when changing the policy.
The published GPUI dependency graph needs Rust 1.88 or newer; project tooling
pins Rust 1.98.
GPUI 0.2.2 depends on `stacksafe` 0.1.x, whose `proc-macro-error2` dependency
emits a Rust future-incompatibility warning. The published `stacksafe` 1.x fix
is outside GPUI's semver requirement, so resolve this through a future GPUI
release rather than a Git or local patch.
Keep direct `nix` on the newest 0.29.x release while `portable-pty`'s
`MasterPty` exposes only `as_raw_fd()`. Nix 0.30 and newer require `AsFd` for
`fcntl`; upgrading earlier would require an unsafe borrowed-descriptor
conversion in `huterm-core`, which denies unsafe code. Remove the matching
Dependabot ignore when a `portable-pty` release exposes a safe borrowed
descriptor.
On macOS, GPUI shader compilation needs Xcode's optional Metal Toolchain;
`mise run doctor` checks it and reports the installation command.
On Ubuntu, GPUI's X11 backend needs both XKB development packages at link time
and a Vulkan device at runtime; CI uses Mesa's software Vulkan driver under
Xvfb.
GPUI's `ShapedLine` contains a large inline decoration buffer. Retained terminal
rendering should cache `Arc<LineLayout>` from `layout_line`, not `ShapedLine`,
and apply colors and decorations during paint. Use the generic `monospace` font
family on Linux; requesting macOS-only Menlo repeatedly exercises GPUI's
missing-font fallback path.
Derive the terminal's grid geometry once through `GridMetrics`. GPUI 0.2.2
reports raw font descent as negative on macOS and Linux, so normalize it to a
positive distance before calculating cell height, baseline, or decorations.
Round grid dimensions in physical pixels using the window scale, then convert
back to logical points. Keep unrounded font measurements for scale changes;
rounding up to logical points makes Menlo 12's grid 14% too wide on Retina.
GPUI normalizes shifted punctuation to its resulting symbol and clears Shift.
Bind reload as `cmd-<` / `ctrl-<`, not `cmd-shift-,` / `ctrl-shift-,`; test
the real `KeyBinding` matcher as well as terminal-input reservation.
Derive scrollbar geometry and label text from the displayed snapshot offset.
Growing the grid pulls rows out of Alacritty history. A scrolled viewport must
adjust its offset for both history growth and shrinkage to avoid resize drift;
offset zero remains pinned to live output.
Protocol selection ranges include both endpoints. Keep a mouse-down anchor
without exposing a range until dragging reaches another cell; use that same
optional range for highlighting and text extraction.
Use the same inset track for painting and drag mapping. Indicator visibility
depends on recent interaction, including at offset zero; advance its fade in
the UI refresh loop so idle terminals redraw it. Keep the label background
opaque before applying the indicator's fade opacity.
Theme reload must invalidate prepared row colors even when the terminal snapshot
is unchanged. Font changes must also invalidate glyph layouts and update PTY cell
pixel dimensions even if the row/column count stays the same. Keep bundled theme
licenses in the packaged resources.
GPUI element `on_mouse_move` filters by hover. Register drag tracking through
`Window::on_mouse_event` during canvas paint to receive movement outside the
window. Transparent macOS titlebars extend the content area; use the shared
terminal viewport inset for PTY sizing, scrollbar geometry, and mouse input.
Within that viewport, `TerminalLayout` owns padded grid bounds and dimensions
for painting, PTY sizing, and selection coordinates. Keep scrollbar geometry
relative to the outer viewport. Clip grid painting to its bounds because a
snapshot from before a resize may arrive after the viewport has shrunk.
Headless Xvfb does not deliver continuous GPUI frames after the initial
presentation. Keep Linux scroll gates based on completion-time snapshot and
queue evidence; require frame-bound paint and input-latency evidence only on a
host that actually delivers enough frames.
Benchmark durations measured with `Instant` include scheduler preemption; label
them as elapsed time, not per-thread CPU time.
Run `mise run license` after any dependency change. GPL and AGPL dependencies,
unknown registries, Git dependencies, and Cargo wildcard requirements are not
allowed.

Desktop structural commands serialize through the Mux mutex on background
workers. Never acquire it from rendering or input callbacks. Terminal clients
send directly to their own runtime, so sibling input and snapshots continue
while another workspace spawns or closes. TerminalView destruction only detaches;
window commands use assessed attachment close to detach or delete the final
view's session.
Use one refresh pump per window to drain bounded batches of tab events. Only the
active tab may begin a snapshot request. Keep ChromeLayout as the shared source
of terminal bounds for painting, mouse input, scrollbars, and PTY resizing.
GPUI close callbacks must return false while asynchronous checks and cleanup run.
Carry close intent across pending operations, and defer application quit until
all pending spawns have published or failed. Foreground checks belong to the PTY
owner, and confirmation must move focus away from terminal action handlers.
Reap the child again after closing the master and joining I/O workers. The last
master descriptor can deliver the hangup that exits the shell; reaping only
before descriptor closure can leave a zombie. Keep this final wait bounded and
return a cleanup error if the child remains alive. This was exposed on macOS
when the execution sandbox rejected process-group signals with EPERM.

Keep a synchronous `on_app_quit` cleanup callback for native termination. AppKit
exits without returning from `Application::run`, and GPUI grants quit futures
only 100 ms. This terminal-only hook is the exception to asynchronous desktop
cleanup: set the shared termination gate before locking Mux, reject queued spawns
after they acquire that lock, and drain existing runtimes before returning.
Initialize scroll benchmark display scale from the attached native window.
Async tab creation may finish after Xvfb's only frame; snapshot benchmark startup
must not depend on TerminalView rendering. Render updates the scale when frames
are available, but Linux snapshot gates must work without them.
Use separate Cargo target directories when comparing baseline and feature
worktrees. Reusing release artifacts across them can retain baseline protocol
metadata; clean the affected local crates if a rebuild reports missing symbols
that exist in the current source.
Global action callbacks run while the dispatching window is borrowed. Defer
Quit routing before updating a native window handle; a synchronous update of
the active window fails with `window not found` even though it remains open.

Alacritty treats mouse encodings 1005 and 1006 as mutually exclusive; the last
enabled format wins. Project its current bits rather than retaining independent
client format flags. Encode mouse events at dequeue time against live modes and
dimensions. GPUI owns physical-button lifetimes and may admit one overflow
release per accepted button, consuming ownership before enqueueing the release.
GPUI's X11 backend remaps Shift vertical wheel lines to horizontal-only deltas.
Restore those deltas to vertical only on the local scroll path on Linux. Leave
application horizontal wheel reports and macOS deltas unchanged until native
macOS device evidence justifies normalization there.
On macOS, GPUI 0.2.2 remaps Control-left independently at press and release,
clearing Control and discarding the original button identity. Match releases
to held logical buttons first; an unmatched Left/Right release closes the held
opposite Left/Right only on macOS. Simultaneous Control-left and physical Right
can collapse into one logical button, so their physical release order cannot be
recovered. Do not extend this fallback to Middle or Linux. Finalize local
selection text on blur as well as release before stopping the drag.
Application mouse coordinates use ChromeLayout's full terminal origin, including
horizontal offsets from vertical tabs. When hiding a retained TerminalView,
release accepted application gestures, finalize selection, and forget canceled
physical buttons: the eventual release may reach a different tab. Ignore stale
pointer callbacks for hidden views; keep focus and window-activation cleanup
subscriptions on each TerminalView.
After bounded PTY cleanup times out, transfer the child handle to a deferred
reaper until wait succeeds; preserve ShutdownTimedOut for the caller. If the
reaper thread cannot be created, emergency synchronous reaping can exceed the
normal shutdown bound. Deferred cleanup cannot outlive OS application termination.
Tab reorder uses source-window capture listeners registered before terminal
mouse handlers. Project movement onto the tab-bar axis and clamp preview/drop
geometry, including release outside the window. Stop propagation when Escape
cancels a drag: GPUI then skips raw keystroke observers. Keep terminal focus
during the drag to avoid false application focus-out/in reports. TabStrip owns
pixel geometry for rendering, reveal, wheel input, and drag slots;
WorkspaceView owns its only scroll offset. Include that offset in drop
mapping. Edge autoscroll runs in the window refresh pump with bounded elapsed
time. Manual scrolling must not pin the active tab. Pass the same preferred
sidebar width to ChromeLayout in WorkspaceView and TerminalView; synchronize
retained views on resize and before activation. Resolve window-size caps
without overwriting the preferred width. Commit canonical Mux order on a
worker and reorder retained views without selecting or recreating them.
Keep the sidebar resize handle entirely inside sidebar bounds. A grab zone
straddling the terminal can start terminal selection or application mouse input
before resize capture consumes release, leaving that terminal gesture stuck.

SessionId, WorkspaceId, and TabId carry a RuntimeId as well as a numeric ID.
Validate the runtime scope on every structural target, including move anchors;
matching socket names do not permit transfers between Mux instances. The numeric
`new` constructors create unscoped IDs for fixtures/restoration input and are
rejected by structural APIs. RuntimeId is unique only within this process; future
IPC needs incarnation identity across processes/restarts. Scope checks prevent
accidental aliasing, not deliberately fabricated IDs.
Keep session/workspace automatic names tied to immutable per-kind creation
ordinals, not membership positions. `None` clears an override; blank custom names
are invalid. Tab title resolution uses the current client title cache without
locking Mux on the UI thread. Desktop tab records are initial snapshots;
propagating later core renames to views belongs with the deferred rename UI.
Moves retain empty parents and never change terminal lifetime. Desktop windows
retain their attachment identity for assessed close. Orphaned spawn cleanup
must preserve resources adopted by another attachment or moved elsewhere. Roll
back newly created sessions on initial workspace/tab failure.

Session AttachmentId is distinct from a terminal RuntimeClient handle. Detach
and retarget preserve zero-view sessions; an assessed last-window close deletes
its session. Desktop close uses prepare/check/commit and request generations.
Run process-table scans off both Mux and terminal-parser threads; carry fresh
assessed background process groups into teardown. For live-shell PTY matching,
normalize macOS ps ttys000/s000 abbreviations before comparing tty names.
Root-shell exit completes the terminal, matching Ghostty, iTerm2, WezTerm, and
Alacritty. Reap through portable-pty immediately, even when history is retained;
do not require an empty OS session or warn about post-exit survivors. Clear
historical cleanup groups after exit, and never signal them when closing
completed history. JobLifecycle lets queued live assessments observe exit or
retirement without looking up old process IDs. Live-shell job warnings remain.
Do not restore post-exit ps/getsid scans: concurrent ps children and unrelated
process churn made ordinary macOS exits require confirmation or retain zombies.
Use explicit FIFO release in the surviving-holder test, never a timed sleep.
Quit captures every session plus window navigation and geometry before cleanup;
retain the first capture through repeated shutdown. The native AppKit bridge
adds only applicationShouldTerminate: to GPUI's existing delegate and vetoes
until assessment, consent, and cleanup finish. Keep unsafe Objective-C calls in
native_quit.rs; run mise run smoke:macos-quit on macOS for real terminate/cancel/
retry/allow coverage. An on_app_quit callback alone cannot cancel Dock Quit.

Allow no-PTY Quit confirmation hosts while quitting, including when a queued
Application request outlives the final Window close. Only new shell windows
remain blocked. Use commit_close_with for capture and termination-gate changes:
validate once before side effects, then capture before teardown. A second
freshness check after capture can strand a half-accepted Quit.
Close consent follows process groups with stable leader creation identity,
including exec and worker churn; leaderless groups need an original surviving
member. New groups or unknown-state widening require reassessment. Use fresh
members for cleanup. Native cancellation tests must send input and observe a
unique shell ACK after cancel and retry; an exited terminal retains snapshots.

The window refresh pump queues shell-exit transitions once for assessed tab close.
Keep automatic exit requests separate from the merged manual close intent, so
canceling one prompt does not lose inactive siblings. Config reload changes only
future exit events. Exited tabs retain history and local selection/scrolling;
close their input queue and reset mouse ownership without emitting releases.
Core drops queued input and terminal replies after root exit, stops the writer,
and keeps final-output parsing, snapshot/selection requests, and emulator resize.
A late writer failure after observed exit must not destroy retained history.

Give confirmation dialogs an explicit viewport-clamped width before measuring
wrapped text. GPUI's w_full/max_w combination can measure a shorter height and
let buttons escape the panel; keep text and button containers nonshrinking.
