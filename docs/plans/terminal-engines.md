# Configurable terminal engines and incremental snapshots

Status: implemented. The later decision to include both engines in every build
supersedes this plan's optional-feature build controls; Alacritty remains the
default. Current build instructions, validation, and measured results are in
[the engine guide](../agents/terminal-engines.md).

## Outcome and scope

Keep Huterm's GPUI renderer and PTY runtime, reduce snapshot work using
Alacritty's damage tracking, then add `libghostty-vt` as an optional engine.
Experimental builds include both engines and select one when each terminal
starts. Measure snapshot improvements separately from engine differences.

The agreed scope is:

- Alacritty remains the default engine.
- Each terminal owns one shared scroll position in the runtime. Independent
  attachment viewports are deferred and must not complicate this experiment.
- Configuration reload changes the engine for new terminals. Existing
  terminals keep their engine, process, and history.
- Both engines produce dependency-neutral Huterm snapshots for the same GPUI
  renderer and use Huterm's PTY ownership and ordered input/write queues.
- Snapshots remain complete and immutable, with unchanged rows shared between
  generations. This experiment does not introduce a delta transport protocol.

Native Ghostty rendering, switching a running terminal between engines,
server/IPC implementation, shared-attachment UX, and new terminal features
such as images or Kitty keyboard input are outside this experiment.

This plan supersedes the client-owned terminal scroll-position requirement for
this experiment. Window navigation, active-tab selection, selection gestures,
and scrollbar animation remain client presentation state. During implementation,
update `AGENTS.md`, `README.md`, protocol comments, and current architecture
guidance to describe the new ownership. Preserve historical plans with an
explicit supersession note where necessary.

## Starting point and evidence

The repository was inspected at commit
`d1fbff37fec625ddb4b0ddb55c4c1d468d4b7fa5` on 2026-09-05.

- `crates/huterm-core/src/engine.rs` contains the Alacritty-specific code. It
  handles byte processing, resize, modes, snapshots, selection extraction,
  and PTY-write/title/bell effects.
- `terminal.rs` serializes emulator access on one runtime thread. PTY reads,
  writes, and cleanup have separate ownership and bounded queues.
- `input.rs` already encodes input using Huterm modes and dimensions. It does
  not depend on Alacritty types.
- `TerminalSnapshot` currently allocates a flat `Vec<Cell>` for every viewport.
  Each cell owns text. The renderer compares snapshot rows to reuse prepared
  rows and glyph layouts.
- Alacritty 0.26.0 exposes `damage()` and `reset_damage()`. Damage is either
  full or a sequence of rows with column bounds. Its current cursor is
  conservatively damaged; damage does not imply different cell contents.
- Ghostty's C render-state API exposes full/partial/clean state and dirty rows.
  Updating render state consumes terminal dirty information; clearing render
  state damage is a separate operation. This is change tracking, not a delta
  message format.

Upstream references are linked below. Ghostty API observations describe the
sources inspected during planning, not a tested native dependency combination.
The implementation must verify them against the selected immutable revision.

## Design

### Engine boundary

Keep a private `TerminalEngine` facade in core with Alacritty and Ghostty
implementations. Use enum dispatch for these two implementations; a public
plugin or trait framework is unnecessary.

The facade owns terminal identity, content generation, retained snapshot rows,
and common publication rules. Adapters own emulator-specific state, modes,
damage extraction, cell conversion, and callback handling. Keep all native
handles and borrowed Ghostty data on the terminal runtime thread. Publish only
owned Huterm data.

The boundary must support:

- Fallible initialization from engine choice, grid, and cell pixel dimensions.
- Ordered PTY byte processing and collection of resulting effects.
- Resize with grid and cell pixel dimensions, including errors and effects.
- Shared viewport movement and actual clamped viewport metadata.
- Snapshot preparation using damage information.
- Existing generation-checked selection extraction and live input modes.

Initialization, resize, and snapshot allocation failures must reach explicit
runtime error paths. Collect callbacks into owned effects; do not perform
blocking PTY writes or reenter the emulator from callbacks. Drain effects from
every operation that can produce them, including resize.

Retain the shared input encoder for the current feature set. Project each
engine's actual modes at dequeue time. Verify that capability replies do not
advertise features Huterm cannot render or encode; configure or suppress such
advertisements where the chosen API permits. Document any remaining mismatch
before claiming interactive compatibility.

### Shared scrolling

Move the canonical viewport offset and scroll application into the runtime.
Use ordered commands for relative movement, absolute positioning, and return
to live output. Keep absolute positions bottom-relative at the Huterm boundary;
convert to each engine's coordinates internally.

Snapshot requests read the current shared viewport rather than selecting a
client-local offset. Returned viewport/history metadata is authoritative.
Preserve the desktop behavior while scrolled: output should retain the viewed
content where possible, offset zero follows live output, and resize/history
trimming clamps the view correctly. Avoid applying both engine anchoring and
Huterm compensation to the same history change.

Keep fractional wheel accumulation and scrollbar animation in GPUI. Adapt the
existing bounded scroll coordinator so coalescing preserves command ordering
and cannot lose a return-to-live request. Continue suppressing application mouse
input while viewing history or while movement away from live is pending.
Local selection and scrollbar dragging remain local gestures that send shared
scroll commands as needed.

Preserve current single-view input-to-live behavior rather than defining future
multi-client arbitration. Shared-attachment event broadcast, differing client
sizes, and simultaneous scrolling policies are deferred.

### Immutable shared rows

Replace the flat cell vector with immutable row objects referenced by `Arc`.
For example, a snapshot can own a `Vec<Arc<TerminalRow>>`, where each row owns
its cells. Exact type names are implementation details.

For each requested snapshot:

1. Read accumulated emulator damage and current metadata.
2. Convert dirty rows into owned Huterm rows; retain references to clean rows.
3. Publish a complete immutable snapshot containing all row references.
4. Clear consumed damage only after the retained snapshot state is coherent.

The renderer uses row identity to skip unchanged row preparation. Do not clone
the complete cell grid elsewhere to restore the old interface. Update selection
painting, coordinate lookup, and benchmark consumers to use the row model.

This reduces per-cell construction and comparison without making delivery
depend on the immediately preceding snapshot. An old snapshot stays valid, and
the newest complete snapshot can replace skipped intermediate snapshots. The
local handoff copies row references rather than all cell contents; this is not
yet a smaller serialized wire format.

Start with whole-row rebuilding for both engines. Alacritty's column damage
can be ignored initially. For conservatively dirty rows, consider retaining the
old row when converted contents compare equal, but measure the tradeoff before
adding comparison work to every full-screen update.

Cursor, modes, history size, viewport, and dynamic colors must update even when
cell rows are reused. Theme/font/selection invalidation remains independent of
row identity. First snapshot, resize, alternate-screen transitions, viewport
changes, and global damage must have a correct full-refresh path. Preserve
existing renderer scroll alignment where it remains valid; do not add scroll
delta operations merely to avoid that fallback.

Keep content generation distinct from viewport/publication changes. Scrolling
must not invalidate a selection merely because a new snapshot was published.
Output and resize retain the existing stale-selection rejection contract.

### Configuration and build selection

Proposed configuration:

```toml
[terminal]
engine = "alacritty" # Or "ghostty" in a build containing the optional engine.
```

Carry a dependency-neutral engine choice through terminal creation. Capture
the choice when the user invokes New Tab or New Window, before asynchronous
work begins. Reload must not change a pending spawn's captured choice.

Unknown or unavailable engines are explicit configuration errors. Do not
silently launch Alacritty after a requested Ghostty engine fails. Reload errors
preserve the running configuration. At startup, an unavailable explicit choice
must not be hidden by the existing configuration fallback behavior.

Add an optional Cargo feature for Ghostty. Alacritty-only builds remain usable
without Zig or Ghostty artifacts. Experimental builds enable both engines and
need no recompilation to switch the default for new terminals. Provide a
discoverable Mise task for launching this build.

Evaluate published safe Rust bindings before introducing local unsafe code.
Pin compatible bindings, native source, and Zig versions together, honoring
the three-day dependency policy. Verify callback lifetimes and render-state
borrows against the selected API. If custom FFI is necessary, isolate it in a
small wrapper crate with a safe interface and a narrowly documented lint
exception; keep core and protocol free of unsafe code.

Prefer static linking for the experiment's packaged binary, subject to proving
both platform builds. Make native sources and build inputs reproducible through
explicit Mise tasks and immutable source identifiers. Do not rely on an
unrecorded system library or let a native fetch bypass source policy. Any policy
exception must be narrow and documented. Audit native transitive dependencies
and retain their notices; Cargo's dependency audit alone does not cover them.

## Implementation sequence and verification

### 1. Capture the baseline and settle native compatibility

Record current Alacritty revision, release build settings, hardware, and the
existing renderer/scroll benchmark results. Add a deterministic headless engine
benchmark through a Mise task, covering byte processing and snapshot conversion
separately. Keep fixture loading and verification outside timed sections.

Check the selected Ghostty bindings/native revision for damage, modes, effects,
selection, and static linking on macOS arm64 and Linux x86_64. A minimal build
and byte-to-cell smoke is sufficient here; do not integrate Ghostty into the
desktop before the Alacritty snapshot milestone.

### 2. Move scrolling into the runtime with Alacritty

Introduce shared scroll commands and return authoritative viewport state in
snapshots. Adapt wheel, keyboard, thumb dragging, and selection autoscroll.
Update ownership documentation alongside this change.

Prove live-follow, pinned output, history trimming, grow/shrink resize, command
coalescing, and mouse suppression during pending scroll. Run the existing
scroll benchmark and native checks. Preserve selection behavior across scroll
and tab switches. Record this intermediate benchmark stage so ownership changes
do not get mistaken for snapshot or engine improvements.

### 3. Use Alacritty damage and shared rows

Introduce the row snapshot model and retained row cache. Consume Alacritty
damage, update GPUI's row reuse, and remove full-grid comparisons from the
normal unchanged-row path.

Focused tests must prove old snapshots remain unchanged, skipped publications
are safe, changed rows update, and cursor/mode/color-only changes remain visible.
Exercise wide cells, combining characters, hard/soft wraps, alternate screens,
resize, and selection extraction. Compare the same benchmark fixtures with the
baseline and the shared-scroll intermediate stage.

### 4. Add Ghostty behind the same contract

Implement the Ghostty adapter, effects, fallible operations, and config wiring.
Create and destroy non-Send native handles on the runtime thread. Coordinate
startup completion so failed engine initialization cannot publish a usable tab
or leak a child, PTY descriptor, reader, or writer. Keep cleanup bounded and
preserve the existing deferred-reaping behavior.

Run the engine contract fixtures against both adapters. Compare observable
cells, styles, modes, cursor, selection text, and replies for the shared feature
set. Record intentional emulator differences instead of forcing universal
byte-for-byte equivalence. Test failed startup and unavailable-engine config,
reload during pending creation, and existing terminals retaining their engine.

### 5. Validate the experiment end to end

Run `mise run check`, `mise run test`, and `mise run license` at the relevant
milestones, then `mise run verify` before implementation handoff. Ensure the
optional feature is actually covered by tests and native packaging, and also
check the Alacritty-only build. CI must cover Linux x86_64 and macOS arm64.

Exercise shells, a full-screen editor, application mouse input, copy/paste,
theme/font reload, tab hiding, shared scrolling, resize, and close/quit with
both engines. Use focused failure tests to prove lifecycle cleanup. Confirm
tests actually run and fail at their intended assertions when test-first or a
proportionate targeted perturbation is needed.

## Benchmark interpretation

Compare these stages on the same host:

| Stage | Engine | Snapshot path |
| --- | --- | --- |
| Baseline | Alacritty | Existing client viewport and flat snapshots |
| Shared scroll | Alacritty | Runtime viewport and flat snapshots |
| Incremental | Alacritty | Damage-driven shared rows |
| Alternative | Ghostty | Damage-driven shared rows |

Use identical bytes, chunk boundaries, grid sizes, and snapshot cadence for
headless comparisons. Include ASCII output, styled text, Unicode, sparse row
updates, full-screen rewrites, scrollback, and resize/reflow. Measure processing
and conversion independently, then measure their combined path. Validate the
result so an engine cannot appear faster by ignoring work.

Report elapsed-time distributions, throughput, rebuilt/reused rows, and memory
or allocation measurements where practical. Label elapsed time accurately;
wall-clock timing includes scheduler preemption. Record actual retained history,
since the engines' history limits and pruning differ. Record native optimization,
CPU/SIMD settings, compression policy, and exact engine revisions. Run timing
trials separately, warm them consistently, and alternate engine order to reduce
systematic bias. Use separate target directories for separate worktrees.

Extend `bench:renderer` and `bench:scroll` to identify the actual engine and
revisions in their output. Use controlled benchmark configuration. Keep existing
queue and scroll correctness budgets; do not weaken them to accommodate an
adapter regression. Set any new numerical performance target from the recorded
baseline before evaluating the candidate.

Doom-fire at 120x40 remains a useful full-screen workload. Its producer-reported
frame rate does not prove displayed frame rate. Sparse updates are the primary
test of incremental snapshot savings. Xvfb's snapshot and queue evidence remains
useful, but sustained presentation and frame latency require a native host that
actually delivers frames.

## Acceptance and rollback

The experiment is ready when both engines run the current desktop feature set,
configuration reliably selects new terminals, lifecycle failures clean up, and
shared snapshots pass the correctness and coalescing tests. Partial-update
measurements must demonstrate whether the expected savings materialize; report
negative results rather than assuming Ghostty or damage tracking is faster.

Alacritty remains the default regardless of benchmark outcome. Disabling the
Ghostty feature removes its native toolchain requirement. The shared-row
optimization can remain independently if it earns its complexity. Switching
configuration back affects new terminals; existing Ghostty terminals must close
normally and cannot migrate their state.

## References

- [Alacritty 0.26.0 damage API](https://docs.rs/alacritty_terminal/0.26.0/alacritty_terminal/term/struct.Term.html#method.damage)
- [Ghostty C API overview](https://github.com/ghostty-org/ghostty/blob/main/include/ghostty/vt.h)
- [Ghostty render-state API](https://github.com/ghostty-org/ghostty/blob/main/include/ghostty/vt/render.h)
- [Ghostty terminal API and effects](https://github.com/ghostty-org/ghostty/blob/main/include/ghostty/vt/terminal.h)
- [Candidate Rust bindings](https://github.com/Uzaaft/libghostty-rs)

## Remaining decisions

No product-scope question blocks this plan. Implementation preflight must select
and prove the exact bindings/native/Zig combination, choose the native source
packaging method, and document any required source-policy exception. If that
combination cannot preserve the current feature contract, report the specific
gap before broadening the experiment or accepting reduced functionality.
