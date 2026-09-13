# OSC 52 clipboard writes

Status: implemented after Ghostty upgrade commit `44def94` on 2026-09-13.
Linux and native macOS clipboard acceptance pass on both engines.
This revision retains the earlier Claude review fixes and supersedes the
proposed custom parser patches and configurable payload cap.

Issue: [#80: Support OSC 52 clipboard writes with client-side policy][issue].
Parent: [#78: Terminal protocol compatibility][compatibility].

## Outcome and scope

Copying in tmux, or in an application that emits OSC 52, updates the macOS or
Linux system clipboard through an attached local Huterm desktop view. Both
engines use the same policy and delivery contract. Manual selection, Copy, and
Paste continue working independently.

Keep escape parsing and terminal ordering in core. GPUI owns permission and
the OS clipboard operation. Each accepted request has one recipient and is
never persisted, broadcast, reassigned after failed delivery, or replayed.

The allow-by-default, write-only policy comes from the issue. Following
review, local attached views remain eligible while unfocused or hidden so a
copy followed by Cmd-Tab does not disappear. Huterm queue budgets are fixed
initial defaults, separate from upstream parser limits.

Do not include clipboard reads, notifications, query-response infrastructure,
shared viewport changes, IPC, or remote clipboard forwarding in this delivery.
Do not change terminal identity as part of this work.

## Evidence that affects implementation

The [matched Ghostty upgrade](ghostty-upgrade-clipboard.md) uses Rust bindings
`5988a0b78b4aa804d1c12e66bbfe662bd97d81c0` and native Ghostty
`22d13172cde98a0a4dda05d3d6a3fcb0dd8ed018`. It supplies upstream allocating
OSC bounds and binary/empty clipboard callback safety. Alacritty 0.26.0 and
VTE 0.15.0 remain unchanged.

- Ghostty caps allocating OSC capture at 8 MiB encoded, approximately 6 MiB
  decoded. Oversized OSC 52 and OSC 1337 Copy requests are dropped, and later
  valid writes recover. Core regression tests cover this behavior.
- Alacritty has no corresponding parser/decode cap. Accept that upstream
  behavior; do not vendor VTE or Alacritty to add Huterm-specific limits.
- Huterm still bounds its own pending effects before retaining or cloning
  payloads. This cannot bound allocations already performed inside Alacritty.
- Ghostty exposes decoded bytes safely; Huterm validates UTF-8 before delivery.
  Its newer native decoder uses SIMD Base64 decoding. Recheck malformed-input
  fixtures against this pin; do not reuse the older decoder's rejection matrix.
- Engine selector and malformed-input differences remain upstream behavior.
  Tests must distinguish shared valid OSC 52 behavior from engine-specific
  rejection and extension support rather than introducing parser patches.
- Clean tmux 3.4 discovers clipboard support with the existing terminal
  identity. Its copy output uses an empty selector and BEL termination.
  Default `set-clipboard external` permits tmux's own copies; application
  forwarding requires `set-clipboard on`.

Primary source seams are Alacritty's `term::clipboard_store`, VTE's
`Parser::action_osc_put` and OSC 52 dispatch, Ghostty's
[`osc.Parser`][ghostty-parser] and [`clipboardContents`][ghostty-clipboard],
and the binding's `ClipboardWrite::contents` and `ClipboardContent::from_raw`.
The [tmux clipboard documentation][tmux] describes the application permission
distinction. Convert the temporary probes into production-path regression
tests rather than depending on temporary artifacts for future validation.

## Behavior and delivery budgets

```toml
[terminal]
clipboard_write = "allow" # "allow" or "deny"; default "allow"
```

Successful reload applies to existing terminals and cancels pending clipboard
requests when permission is removed. Invalid reload retains the previous
configuration. This setting does not restrict explicit Copy or Paste commands.

| Input or condition | Behavior |
| --- | --- |
| Empty selector or exactly `c` | Target the system clipboard |
| Engine-normalized destination | Deliver only system-clipboard writes; retain upstream selector parsing |
| Valid Base64 containing valid UTF-8 | Replace clipboard text exactly |
| Empty encoded payload | Replace with empty text, clearing previous text |
| Read request `?` | Ignore; never read or reply with clipboard contents |
| Invalid UTF-8 callback data | Ignore without changing the clipboard |
| Malformed Base64 or extra parameters | Preserve upstream parsing behavior |
| Payload over upstream capture or Huterm queue limits | Drop the request without truncating |
| BEL or ESC-backslash ST | Finish one logical request, regardless of read boundaries |
| CAN/SUB | Preserve current engine behavior for within-limit requests; discard an already oversized request |
| No eligible recipient or exhausted queue budget | Drop immediately without retry |

Changing CAN/SUB semantics for otherwise valid requests is deferred. Recheck
the upgraded pin's behavior in the framing fixtures. Ghostty overflow tests
must prove that cancellation/reset cannot dispatch a previously rejected
prefix. Alacritty has no upstream capture limit to assert. Preserve embedded
NUL bytes in the engine-neutral text;
native tests must verify exact handling rather than silently truncating it.

The [xterm specification][xterm] supports broader selector behavior and
clearing on malformed data. Document Huterm's narrower contract rather than
claiming complete xterm equivalence. Unknown selectors may already have been
normalized to a standard-clipboard callback; Huterm cannot recover that raw
selector and will accept the normalized destination. Retain Ghostty's OSC 1337
Copy support
through its existing clipboard callback. Apply the same permission, UTF-8
validation, queue budgets, and recipient rules to it.
Alacritty may continue to ignore that extension; engine parity is required for
OSC 52, not for every engine-specific protocol.

Use upstream parser/decode limits without a Huterm payload-size setting.
Bound Huterm-owned queues independently: at most eight pending effects and
16 MiB per terminal, and 32 effects and 32 MiB per desktop process across all
its windows and views. Reject a write that cannot fit before retaining another
copy. Count retained allocation capacity, not just logical text length, and
count empty clears against the effect-count budget. Reserve terminal and
process capacity as one admission operation, rolling back either reservation
if the other fails. Carry one reservation through adapter, runtime, and client
staging so moving an effect neither resets its budget nor duplicates its text.
Release capacity on every consume/drop path and preserve accepted order.
These queue budgets do not imply a uniform engine payload limit. No payload
appears in logs. A configurable parser limit remains deferred to upstream.

## Ownership and delivery contract

| Owner | Responsibility |
| --- | --- |
| Upstream engine parser/decoder | Recognition, framing, decoding, and engine-specific limits |
| Engine adapters in `huterm-core` | Convert validated callbacks into owned, bounded Huterm effects |
| `huterm-protocol` | Dependency-neutral effect, destination, origin, recipient, and sequence metadata |
| Core runtime and attachment lifecycle | Validate registration, select one recipient, order delivery, revoke stale registrations |
| GPUI | Publish local eligibility, apply configuration, recheck delivery validity, call the clipboard API |

Add a dedicated effect-delivery path instead of putting payloads into the
existing shared `TerminalEvent` receiver or snapshots. Use a clipboard-write
variant in an extensible effect enum, without implementing other effect kinds.
Keep an empty write explicit. Retain terminal identity, normalized
destination, and an ordered effect sequence; payload Debug output must not
expose the text.

Register a recipient against a validated session attachment and terminal, with
its capability, connection origin, writable status, and revocable generation.
Attachment membership is checked off the UI thread through existing core
ownership. Cloning `RuntimeClient` must not register another recipient. Bind
registration lifetime to attachment detach, retarget, removal, and view
teardown; an arbitrary terminal handle alone cannot acquire clipboard
authority.

GPUI publishes attachment, permission, and focus-recency changes. Any live,
writable local embedded desktop view registered for the originating terminal
is eligible under `allow`, including inactive tabs and hidden, minimized, or
unfocused windows. Opening the palette, showing a confirmation dialog, and
switching apps do not revoke clipboard permission. Scrollback position is also
irrelevant. Detached, read-only, disconnected, TUI, and remote registrations
are ineligible in this version. SSH output inside a local terminal remains
local terminal output; it is distinct from a future remote Huterm connection.

Select at most one eligible recipient at the runtime's effect-admission point.
Use runtime-ordered focus updates only to break ties between eligible views of
the originating terminal. Retain each registration's last-focus ordinal on
blur; use registration order as a deterministic fallback for views never
focused. Do not reuse grid-size control as clipboard ownership. Background
writers can replace the clipboard under `allow`; document that deliberate
behavior and the `deny` opt-out rather than silently dropping their requests.

Stamp the selected registration generation onto the delivery. Revoke it on
permission denial, detach, retarget, terminal membership invalidation, or view
teardown, even if an owner-thread update is still queued. Focus/visibility
changes do not revoke it or redirect an already selected write. The client
checks the generation immediately before the OS operation. Serialize local
permission revocation and clipboard invocation on the GPUI thread. A write
already invoked before revocation cannot be undone; revocation cancels
pending writes, not past OS operations. Drop stale
deliveries; never transfer them to another recipient.
Reattachment/reconnection starts a fresh generation with no old payloads. Core
must invalidate registrations when canonical membership changes too. Publish
initial registration before advertising readiness for copy; requests received
before a valid registration exists are dropped.

Preserve request order within a terminal's PTY processing stream and carry
that order through delivery. No global ordering across independent terminals
is promised. Parsing must never wait for an OS clipboard operation or a slow
client. Bounded admission may fail; lifecycle revocation must remain effective
even when the ordinary message queue is full. Clipboard failure never becomes
a fatal terminal error or an automatic retry. GPUI's clipboard API may not
confirm eventual ownership, so promise at-most-once invocation, not guaranteed
delivery.

## Implementation ownership

The parent owns interface decisions, integration review, and verification.
Fresh-context `gpt-5.6-sol` agents at medium effort implement bounded steps,
self-review their changes, and report exact test evidence before handoff.
Core/protocol, engine adapters, desktop/configuration, and native smoke tooling
have separate file ownership. Integration follows the accepted core API.

The desktop creates one `DesktopHostEffectClient` budget account in its shared
runtime and clones it across all registrations. Each terminal has a private
sink. Only Mux can create a recipient after validating attachment membership;
client handles cannot grant that authority. Non-clone delivery objects retain
capacity reservations until consumption/drop, including after dequeue.

## Implementation sequence

### 1. Confirm the completed upgrade baseline

Upgrade commit `44def94` supplies the matched Rust/native sources. Its core
regressions, full verification, Linux input smoke, and Ghostty scroll benchmark
have passed. Native macOS validation remains outstanding. Preserve native
source hashes,
portable CPU targeting, private source staging, and the native memset
regression.
No clipboard-specific native patches or new VTE/Alacritty vendor entries are
needed. Run `mise run vendor:check` and `mise run ghostty:check` for
reproduction.

### 2. Add shared engine contracts and bounded adapters

Extend `crates/huterm-core/src/engine.rs` contracts and the two engine
adapters. Use the same input fixtures and assert observable effects, not
private parser state. Add tests before enabling delivery so rejection
differences are explicit in the assertions. Keep shared success fixtures for
UTF-8, empty clear, ordered writes, and ignored reads; use engine-specific
expectations for malformed Base64, selector lists, and extra parameters.
Include the exact captured tmux
form `ESC ] 52 ; ; <Base64 UTF-8> BEL` as a deterministic fixture independent
of a local tmux installation.

Retain the upgrade's core regressions for safe empty/binary callbacks and
oversized OSC recovery. Test the admission budgets owned by Huterm separately,
including one over-budget callback and floods of small writes. Do not claim
bounded Alacritty parser memory or introduce dependency allocation tests for
limits that Huterm does not implement.

Intercept `ClipboardStore` inside Alacritty's `EventProxy::send_event`, before
its unbounded event channel. Move the already-owned string into bounded
admission or drop it. Ignore `ClipboardLoad` without invoking its reply
formatter. Ghostty's `on_clipboard_write` must validate the normalized standard
location, empty clear or `text/plain` representation, and UTF-8 before reserving
capacity and copying borrowed bytes. Return the appropriate callback status
on rejection. Neither engine should stage clipboard data in its ordinary
unbounded event vector/channel.

Use one admission path for both adapters, supplied by the terminal runtime;
callbacks must not acquire Mux or wait for GPUI. Exercise many small writes in
one process call as well as one large fragmented write. Preserve non-clipboard
effects and ordering. Queue rejection is nonfatal. Do not promise recovery
from arbitrary process allocator exhaustion inside either upstream engine.

### 3. Implement recipient registration and delivery

Add the protocol effect types and a small core delivery module. Integrate it
with `terminal.rs` and attachment lifecycle code, keeping the Mux mutex out of
rendering and input callbacks. Implement the registration/generation contract,
capacity accounting, deterministic selection, and nonblocking consumption.

Test two eligible views, cloned runtime handles, capability denial,
unsupported connection origins, focus handoff without loss, terminal moves,
detach/retarget, teardown, full queues, and reconnect. Use synchronization
barriers to drive races rather than sleeps. Assert at-most-once delivery, no
recipient fallback, no stale replay, and continuing PTY input/output under a
stalled client.

### 4. Wire GPUI policy and configuration

Add the setting to `huterm-config`, desktop defaults, parsing, reload, and the
generated schema. Add view-local registration handles and a small delivery
consumer using GPUI's existing clipboard API. Publish eligibility from
existing attachment/policy transitions. Keep the consumer active for hidden
and inactive views; it must not depend on rendering or focus. Track focus only
for recipient selection, without reading other `WorkspaceView` entities. Avoid
locking Mux on the UI thread.

Test defaults, explicit denial, invalid configuration, reload cancellation,
and that explicit Copy/Paste remain available under denial. Regenerate and
check the schema with the existing Mise tasks. Add diagnostics using reasons
and counts, never clipboard contents, and avoid per-request notification spam.

### 5. Verify native behavior and document the contract

Add discoverable `smoke:linux-clipboard` and `smoke:macos-clipboard` tasks,
reusing the existing native input harness where practical. Wire build
dependencies and execution into the corresponding CI steps and shared
smoke-process supervision. Linux uses an isolated Xvfb display and an
independent `xclip` process to read the actual CLIPBOARD selection from
Huterm. GPUI's in-process clipboard read can return cached text and is not
sufficient evidence. macOS preserves/restores the clipboard and uses an
independent native reader process, including length-aware reads for NUL
coverage. Compare exact bytes, including empty data, without trim.

Declare `xclip` and `tmux` in Linux smoke prerequisites and the development
guide; install `tmux` explicitly for macOS smoke jobs too. Required CI
acceptance fails on a missing tool instead of silently skipping the tmux case.
Keep engine fixtures runnable without these binaries. Use a private tmux
socket and clean configuration; do not access existing sessions or modify the
user's settings. Test copy followed immediately by switching apps, copies from
inactive tabs, hidden/minimized windows, and copies while the palette holds
focus. Those writes must still reach the clipboard under `allow`; denial and
detach must still drop.

Document the setting, supported selectors, limits, background-copy behavior,
drops on ineligibility, tmux's `external` versus `on` distinction, and the
explicit remote/TUI policy. Record native evidence and any residual platform
limitations in the delivery. Run `mise run license` for dependency changes and
`mise run verify` before handoff, with no active vendor edit sessions left
behind.

## Verification and acceptance

| Risk or requirement | Required evidence |
| --- | --- |
| Framing and engine parity | BEL/ST, every two-chunk split of small fixtures, byte-at-a-time input, and large boundary-crossing chunks on both engines |
| Upstream bounds and recovery | Ghostty encoded-capture boundary and oversized unterminated input, followed by terminator and valid output; no Alacritty allocation-bound claim |
| Huterm admission bounds | Queue bytes/count minus one, exact, and plus one; empty clears consume count; multi-terminal process budget; all drop paths release reservations |
| Partial or malformed writes | Bad Base64/UTF-8, extra parameters, selector lists, unknown selectors, cancellation, NUL, empty clear, and Ghostty OSC 1337 Copy acceptance/rejection under the same delivery policy |
| Duplicate or stale effects | Competing recipients, generation revocation races, permission reload, read-only/TUI/remote states, detach and reconnect; background copies remain allowed |
| Flooding and backpressure | Byte and count budgets across adapter/runtime/client queues; ordinary input/output continues; rejection releases capacity |
| Snapshot/history isolation | Clipboard-only sequences add no payload to snapshots; no reconnect backlog; effects never enter restoration capture types |
| Native clipboard | Exact Unicode, empty text, embedded NUL, multiple ordered writes, sentinel unchanged after rejection, both engines on macOS and Linux |
| Actual daily workflow | Direct OSC 52, clean tmux copy mode, application output under tmux on/external, and herdr's real Copy action |
| Regression | Existing manual Copy/Paste and native input smokes continue passing |

The existing restoration implementation does not yet persist history. Verify
current type/capture separation and leave end-to-end restored-history
execution tests to #43 rather than pretending a snapshot test proves
persistence safety. Linux evidence cannot substitute for native macOS
clipboard evidence. If exact NUL handling is unsupported by a platform API,
resolve and document that contract explicitly before marking the native
acceptance complete; never claim an exact copy based only on a prefix match.

## Related issues and alternatives

- [#77][advertising]: preserve the tested identity and `COLORTERM` hint. Keep
  clean-tmux discovery coverage; broader color advertisement remains separate.
- [#22][viewers] and [#97][windows]: expose explicit registration and
  eligibility data so their eventual attachment/window models can supply it.
  Do not implement shared-view sizing or the complete client window model
  here.
- [#81][notifications] and [#82][queries]: reuse the effect transport seam
  later, but keep notification eligibility and query response correlation
  separate.
- [#44][ipc], [#48][tui], and [#49][remote]: preserve neutral metadata,
  capability denial, revocation, and no replay. No serialization dependency or
  remote permission grant is introduced by this plan.
- [#42][restore] and [#43][history]: no payload belongs in saved state.
- [#98][hierarchy]: structural projection events remain distinct from effects.

A direct callback-to-GPUI queue still needs validated recipient ownership and
bounded retention. An engine-wide allocator
quota would conflate clipboard rejection with terminal grid/history
allocation. A new request/reply framework would pull #82 and #44 into this
feature. Use upstream parsers and the one-way delivery contract above.

Ship one coherent feature PR, with the engine upgrade, core delivery, and
desktop integration separated into reviewable changes. The setting provides a
runtime opt-out; disabling it cancels pending effects while upstream parser
behavior remains unchanged.

## Implementation evidence

The core, engine adapters, desktop integration, configuration, schema, and
native smoke tasks are implemented. Sol agents at medium effort completed
bounded implementation tasks and self-reviews; parent review caught and
verified fixes for release-only sink initialization, startup permission
fallback, and smoke synchronization. No additional runtime crates or vendor
patches were needed beyond the preceding Ghostty upgrade.

- Initial `mise run verify` passed: 123 core tests, 320 GPUI tests, 5 config tests,
  16 protocol tests, 2 GPUI integration tests, and 357 scripting tests.
  Three existing core benchmarks remain ignored. Focused clipboard tests
  also passed in release mode.
- `mise run smoke:linux-clipboard` passed with independent X11 clipboard
  reads for both engines: Unicode, empty text, NUL, ordered writes, rejected
  reads and invalid UTF-8, background/minimized/inactive views, palette focus,
  deny reload, manual Paste under deny, and Ghostty OSC 1337 Copy.
- Clean tmux 3.4 sessions passed real copy mode and `set-buffer -w` on both
  engines. Application writes stayed blocked under `external` and reached
  the clipboard under `on`.
- Installed Herdr 0.9.0 passed its real Copy action on both engines. Private
  config/state directories and sockets isolated its server/client session;
  an SSH environment forced its OSC 52 path. Native `Ctrl+B [` followed by
  `0 v $ y` copied each fixture's exact text, independently read with `xclip`.
  The test closed its own sessions and left existing user sessions untouched.
- `mise run smoke:linux-input` passed exact PTY input checks on both engines.
  Script checks were rerun after native harness fixes.

Clipboard-only data stays outside snapshots and restoration capture types.
Recipients are attachment-validated, queued and popped reservations retain
their budget, and generation checks prevent delivery after revocation.
Startup fallback preserves an explicit valid deny policy; invalid clipboard
policy values stop startup. Invalid reloads retain the working configuration.

The [native macOS clipboard CI step][macos-acceptance] passed at `3f58e44`.
It verified exact Unicode, empty text and NUL, ordered writes, ignored reads,
invalid UTF-8, copy followed immediately by hiding, writes while hidden,
startup denial, and real tmux copy/forwarding on both engines. Ghostty's
OSC 1337 Copy also passed. A dedicated smoke example invokes the production
Hide command; a separate AppKit witness observes hidden state and clipboard
bytes. Inactive-tab, palette-focus, and reload shortcut variants still need
manual native macOS evidence as described in the development guide.

## Unresolved questions

No additional product decision blocks implementation. Queue budgets are
implementation defaults, distinct from upstream payload limits.
Making Ghostty the default remains a separate decision; this plan supports both
engines with their current default unchanged.
Herdr's installed version and real Copy action have been verified on Linux.

[macos-acceptance]: https://github.com/jimeh/huterm/actions/runs/34740217873/job/103678555571

[issue]: https://github.com/jimeh/huterm/issues/80
[compatibility]: https://github.com/jimeh/huterm/issues/78
[advertising]: https://github.com/jimeh/huterm/issues/77
[viewers]: https://github.com/jimeh/huterm/issues/22
[notifications]: https://github.com/jimeh/huterm/issues/81
[queries]: https://github.com/jimeh/huterm/issues/82
[ipc]: https://github.com/jimeh/huterm/issues/44
[restore]: https://github.com/jimeh/huterm/issues/42
[history]: https://github.com/jimeh/huterm/issues/43
[tui]: https://github.com/jimeh/huterm/issues/48
[remote]: https://github.com/jimeh/huterm/issues/49
[windows]: https://github.com/jimeh/huterm/issues/97
[hierarchy]: https://github.com/jimeh/huterm/issues/98
[ghostty-parser]: https://github.com/ghostty-org/ghostty/blob/22d13172cde98a0a4dda05d3d6a3fcb0dd8ed018/src/terminal/osc.zig
[ghostty-clipboard]: https://github.com/ghostty-org/ghostty/blob/22d13172cde98a0a4dda05d3d6a3fcb0dd8ed018/src/terminal/stream_terminal.zig
[tmux]: https://github.com/tmux/tmux/wiki/Clipboard
[xterm]: https://invisible-island.net/xterm/ctlseqs/ctlseqs.html
