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
- `huterm-core` alone owns PTYs and canonical emulator state. One runtime
  thread mutates each terminal. Blocking PTY reads, ordered writes, and child
  waits stay off the GPUI thread.
- `huterm-gpui` owns macOS/Linux window state, rendering, key translation,
  focus, selection gestures, and scrollbar animation. The terminal runtime owns
  the shared viewport; clients send ordered scroll commands.
- Alacritty is the default; every build includes `libghostty-vt`, which shares
  the same owned rows and PTY/input runtime. Do not use either engine's
  application or PTY event loop.
  Do not copy or depend on GPL-covered Zed application or terminal-view code.
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
tools for each job. `mise.lock` also records the Rust version, so run
`mise install` and commit the lock after any toolchain bump; otherwise CI
fails to resolve the tool.

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
Validate `when` with `KeyBindingContextPredicate::parse` before building a
`KeyBinding`; `KeyBinding::new` unwraps its predicate parse. Reserved
keystrokes derive from the compiled keymap and compare modifiers plus `key`,
never `key_char`, so `alt-r` stays reserved when macOS reports `®`. Multi-key
bindings reserve only their first stroke; GPUI owns pending sequences and matched
final strokes. Later strokes typed alone must reach terminal input. A conditional
single-key binding reserves nothing, or `ctrl-c` with `when = "selection"` would
swallow interrupts when the predicate is false. Per-scope
wrapper actions (`InvokeApp`, `InvokeWindow`, `InvokeTerminal`) route catalog
commands; anything that needs a window, such as about and open_settings, must
be Window scope because global action handlers receive only `App`. Runtime
rename commands fill omitted tab/workspace targets on the UI thread and resolve
the session under the Mux lock on the worker. Rebuild menus on reload so macOS
shortcut display follows user bindings. GPUI menus show the earliest binding
for an action and equal-depth dispatch ties go to the latest, so the compiled
keymap lists unconditional user bindings first and conditional ones last.
GPUI's `Not` predicate checks every ancestor context, so
`Terminal && !confirming` works across the Workspace and Terminal stack.
Derive scrollbar geometry and label text from the displayed snapshot offset.
Growing the grid pulls rows out of Alacritty history. Let the runtime engine
anchor its shared viewport across output and resize; never also compensate the
client's offset for history growth or shrinkage. Offset zero follows live output.
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
Ghostty builds use libghostty-vt/sys 0.2.1, native revision
`a887df42c56f6de86c0fe6da9c4eeca37931e083`, and Zig 0.15.2. Run
`mise run ghostty:prepare` before direct Cargo build commands; it checks the
full native source tree against `scripts/ghostty-source.json`. Keep that source,
the binding versions, and bundled notices aligned. All builds include both
engines and require the pinned Zig toolchain. Native source dependencies use
Zig's content hashes; their notices are in `third-party/ghostty` because they
are outside Cargo's license audit.
At this native pin `max_scrollback` is bytes despite the published binding/header
claiming lines. The adapter uses 16 MiB and reports actual retained rows.
Ghostty color-only OSC updates can leave render rows clean. Compare effective
colors and retain explicit palette override information before consuming damage.
Its API lacks the override mask; the adapter probes changed defaults after
OSC/RIS invalidation hints, then restores defaults before rendering. Keep this
hint state across input chunks, including snapshots between fragments.
Construct non-Send native handles on their owner thread before spawning the PTY;
only publish startup after workers are ready. Scroll-and-snapshot share one
ordered control operation. Ordinary snapshots must not reset the viewport.
Complete snapshots share immutable `Arc<TerminalRow>` values; never clear engine
damage before owned cache state is coherent. Content generation excludes scrolling.

Ghostty's public mode bits can disagree with its active mouse format/tracking.
Read the active behavior through the retained native mouse probe, with synthetic
200x200 geometry independent of the real grid, then feed Huterm's shared encoder.
Never send probe output to the PTY. At the selected native pin, disabling an
inactive format resets to legacy encoding; Alacritty preserves the active format.

Shared relative scroll requests can follow output that advances the runtime
viewport. Benchmark expected offsets use command plus pre-operation runtime
viewport/history; retain the client prediction separately. Clamp pending relative
UI intents after an authoritative completion so a saturated history boundary
does not leave scroll debt that affects later reversal.

Treat malformed TOML and invalid engine choices as fatal at startup.
For valid TOML with a known engine, fallback from unrelated settings
errors must preserve that engine. Fatal snapshot errors set the runtime closing
gate; latch client snapshot failure so pending scroll or invalidation cannot
create an immediate retry loop. Set Ghostty device attributes explicitly: the
pinned native implementation answers a callback returning None despite binding
documentation saying it suppresses replies.

Keep verified native source inputs in `.native/ghostty`, outside Cargo's
`target` directory. The pinned rust-cache action recursively removes non-Cargo
files under `target` before saving, leaving incomplete native source trees on
restore. Preserve source hash checks; never repair mismatches silently.

Run Xvfb with `-noreset` in desktop harnesses. A last-client disconnect otherwise
resets the server and sends another SIGUSR1 to xvfb-run, which can interrupt its
cleanup wait and return exit 5 despite successful file removal. Keep application
exit checks and benchmark budgets strict. Clear any satisfied pending scroll
intent after authoritative completion, including absolute/live intents, so
later invalidation cannot replay a stale target. Cargo must force the verified
native source path and benchmark optimization over inherited environment values.

Build commands use `scripts/build-exec.sh` to select Xcode 26 when the default
macOS SDK is 27 or newer. Zig 0.15.2 otherwise fails linking its own build runner
with undefined system symbols before compiling Ghostty. Preserve explicit
`DEVELOPER_DIR` overrides; route new native build tasks through this wrapper or
`mise run build:exec -- <command>`. Preparation cannot export this environment
to a later Cargo task, so each native build invocation needs the wrapper.

Repository scripts use Bun with TypeScript 7 for type checking, and Bash for the
SDK wrapper. Pin Bun and Zig in Mise and JavaScript dependencies in bun.lock;
keep bunfig.toml's minimum release age aligned with the three-day policy.
Run `mise run check:scripts` for tooling edits. Native source preparation uses
Bun FFI only for the OS-owned `flock`; retain automatic lock release on process
exit and the existing top-down source-tree hash order. Test changes to extraction
and locking on macOS and Linux. The Linux FFI library is glibc, matching
Ubuntu CI.
Buffer the native archive response before passing it to `Bun.write`. Bun 1.4.0
can stall on Linux when writing the live HTTPS response directly, even though
local HTTP fixtures pass. Verify download changes with a cold preparation run.
The local sys 0.2.1 patch backports the upstream CPU-target option and fixes its
crate-relative build-script watch path for vendoring. Cargo
forces `LIBGHOSTTY_VT_SYS_CPU=baseline` for portable native instructions.
Keep bindings and the native revision unchanged; remove the patch
when a reviewed published release supplies the fix. See
`third-party/vendor/README.md` for provenance. The path dependency has a distinct
Cargo fingerprint from the old registry crate; no manual cache cleanup is needed.
Local builds retain warm native artifacts. The pinned CI cache action prunes
path dependencies inside the repository, so CI rebuilds the vendored sys crate.
Preserve upstream formatting in vendored crates. The staged Rust formatter
excludes `third-party/vendor`; Cargo still compiles it as a dependency.

GPUI's macOS `key_char` resolves Option dead keys separately, and AppKit handles
marked text before GPUI shortcuts. Keep Option-only composition in caller-owned
`UCKeyTranslate` state after GPUI dispatch; never create native marked text for
Option dead keys. Use the selected input source's Unicode layout and leave
input methods without one on the native text path. Cancel local Option state on
commands, pending shortcut prefixes, focus/tab/policy changes, and layout changes.
GPUI replays unmatched chord prefixes through `on_key_down` and then its input
handler, bypassing raw keystroke observers. Track the actual pending sequence and
consume replay keys before they become text, including nonprinting strokes.
Timeout announces no pending input before replay; mismatch announces it after.
A replayed fallback action reports only its matched prefix's last key. Capture
GPUI's enabled fallback bindings when it resolves a sequence, not when the
prefix starts: selection can change conditional eligibility without changing
focus. Capture at timeout's pre-replay notification or the first wrapper action's
capture phase, then freeze through all replay actions, including keymap reloads.
Use GPUI's matcher to consume whole prefixes, including repeated keys. Public
keystroke interceptors omit modifier-only mismatches, so do not rely on them for
this capture. Ignore menu actions while GPUI still holds a pending sequence.
The later pending notification clears completed actions. Put shorter fallback
bindings before longer chords in config:
a newer short binding makes GPUI discard the longer pending match.
Mirror GPUI's cancellation on focus change;
window deactivation alone leaves its pending sequence alive. Native commits must
never be classified by their text. Accepted replay actions still skip the current
native event lookup.
`UCKeyTranslate` can emit text while starting another dead key, and completed
state can remain nonzero. Probe Space with NoDeadKeys on copied state and compare
against zero-state output; never send probe text to the PTY or decode state bits.
Serialize all native layout tests with their shared mutex. Parallel Carbon calls
caused SIGSEGV; the SDK marks `LMGetKbdType` as not thread safe. Production calls
stay on the main thread.
Route other printable input through `EntityInputHandler`. Consume recognized raw
Meta/Control/special chords even when their input queue is full, or AppKit can
insert a second interpretation. Cancel native preedit on the exact NSView outside
GPUI's update borrow, and skip deferred cancellation when the active view already
has newer preedit. Bindings and menu installation share one operation;
`smoke:macos-menus` reads actual NSMenuItem shortcuts.
Use XTest for `smoke:linux-input`: xdotool's `--window` path uses XSendEvent and
does not exercise the server's XKB modifier state. The smoke explicitly unbinds
Alt-3 because Linux reserves Alt-1 through Alt-9 for tab selection.

GPUI's native menu matcher uses a fixed Workspace/Pane/Editor context. Menu
ordering fixtures need a predicate true there, such as `!confirming`; `Terminal`
never participates and cannot detect conditional bindings moving ahead of defaults.

Use `timeout --foreground` around raw-PTY readers in desktop smoke fixtures.
Without it, GNU timeout puts the reader outside the terminal foreground process
group, so accepted terminal input never reaches the fixture reader.
