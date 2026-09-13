# Ghostty-only terminal engine

Implement [issue #118](https://github.com/jimeh/huterm/issues/118) as a separate
delivery before the combined capability and query work in #77 and #82. Every
launch must use Ghostty, while core's private engine module continues to hide
native emulator details behind Huterm-owned inputs, effects, and snapshots.

Status: approved implementation contract. This document
authorizes no additional protocol behavior. Planning has inspected source; it
has not established a passing runtime baseline.

## Delivery ownership

Use the user-selected `ship-feature-pr` workflow through a reviewed, ready PR.
Do not merge, deploy, or release as part of this delivery.

| Role | Assignment | Responsibility |
| --- | --- | --- |
| Orchestrator and verifier | Parent agent | Settle scope, inspect all changes, assess evidence, own Git and PR operations, reconcile reviews, and verify readiness. |
| Implementer | `gpt-5.6-sol`, medium effort | Implement the contract and focused tests, report evidence and gaps, and handle settled corrections. |
| Codex reviewer | Fresh `gpt-6-astra`, low effort | Independently review the pushed exact base/head pair under `dual-review`. |
| Claude reviewer | Independent Claude channel under `dual-review` | Review the same contract and revisions without seeing the Codex report. |

Start the implementer and native reviewers with `fork_turns="none"`. Supply a
self-contained brief naming the approved plan, checkout, revisions, constraints,
ownership, allowed actions, and expected evidence. Prohibit further native or
CLI model delegation. Do not substitute models or efforts silently if a selected
configuration is unavailable. Use the Claude review skill's configured default;
the user has not selected a Claude model override.

The single implementer has exclusive mutation ownership of the delivery checkout
until handoff. It does not commit, switch branches, push, or create PRs. The
orchestrator may inspect during implementation but must not edit, mutate Git, or
run potentially mutating verification concurrently. Reuse the implementer session
for corrections when practical. Use a disposable checkout for destructive test
perturbations after capturing the implementation, with separate build artifacts.

The delivery home is `/Users/jimeh/.t3/worktrees/huterm/t3code-f1df3ef5`, a linked
worktree on `t3code/ghostty-only-engine`. Planning began at
`0b73e7d4e47eb8a6f79c563d86bf45ff92f9219f` with no staged, unstaged, or untracked
changes. Refresh and record the complete intake state and worktree mapping before
implementation. Preserve unrelated work and other worktrees.

## Implementation contract

### Engine and runtime boundary

- Replace the dispatch enum in `crates/huterm-core/src/engine.rs` with a concrete
  wrapper around the private Ghostty adapter. Retain engine-owned viewport
  prediction and link resolution. Keep fallible operations and existing error
  propagation; Ghostty initialization and snapshot failures remain meaningful.
- Remove `engine/alacritty.rs`, Alacritty dispatch, `TerminalEngineKind`, and
  engine selection from launch commands, terminal clients, Mux, GPUI, fixtures,
  and benchmarks. Preserve useful engine/revision diagnostics as fixed metadata,
  without keeping selectable state in runtime or protocol APIs. Source runtime
  and benchmark revision logs from the existing `GHOSTTY_REVISION` constant.
- Keep native handles and types inside core's private engine integration. Keep
  `huterm-protocol` dependency-free and retain owned immutable snapshots. PTY
  spawning, input encoding, lifecycle, attachment authority, and rendering remain
  independent of native emulator representation.
- Preserve owner-thread initialization before child launch, ordered runtime
  operations, snapshot failure latching, retained history, and bounded cleanup.
  Do not add a backend trait, one-variant selector, or fallback implementation.

A direct re-export of the adapter is possible, but would require moving the
wrapper's viewport and link behavior. The concrete wrapper keeps this migration
focused and leaves emulator integration in one private module.

### Configuration compatibility

Retain an optional legacy `terminal.engine` field only in configuration parsing.
Distinguish omission from an explicit value. Newly generated configuration omits
the field; the generated schema documents it as deprecated and accepts only the
two previously valid strings.

| Input | Startup | Reload |
| --- | --- | --- |
| Omitted | Use Ghostty without warning. | Accept. |
| `"ghostty"` | Use Ghostty without warning. | Accept. |
| `"alacritty"` | Use Ghostty with a migration warning. | Accept with a migration warning. |
| Other string or non-string | Reject startup. | Reject and retain the current configuration. |
| Malformed TOML | Reject startup. | Reject and retain the current configuration. |

Recommended warning: `terminal.engine = "alacritty" is deprecated; Huterm now
uses Ghostty. Remove terminal.engine from your configuration.` Report it once
per load/reload attempt, not for each created terminal. Keep warning diagnostics
separate from errors so a valid legacy configuration does not activate fallback.
Do not rewrite the user's file.

Warnings must not hide fallback errors or keymap conflicts. A failed reload
shows its error while retaining the active configuration. A later successful
reload with omitted engine or explicit `"ghostty"` clears the legacy warning.
Test this lifecycle because the current UI stores a single configuration status
and clears it during successful reload.

Preserve the existing unrelated-setting startup fallback and its clipboard
policy recovery. Validate the legacy field on that path too: a bad font must not
hide an invalid engine value, discard the migration diagnostic, or turn an
explicit clipboard deny into allow. Reload remains transactional and does not
fall back to defaults on invalid input.

### Coverage and tooling

- Convert shared parity tests to Ghostty contracts. Before deleting adapter-local
  tests, map their useful behavior to remaining tests and migrate uncovered
  cases. Do not preserve duplicate tests merely by moving them.
- Keep current Ghostty expectations for inactive mouse-format resets, device
  attributes, OSC 8 destination bounds, and OSC 52 selector/malformed-input
  behavior. Preserve current OSC 1337 clipboard handling without expansion.
- Collapse duplicate smoke and benchmark engine loops and selectors. Normal
  smoke configuration must omit `engine`, proving the default launch path;
  dedicated configuration tests exercise the legacy values. Retain one
  launch-level compatibility check with `engine = "alacritty"` that verifies
  the actual runtime reports Ghostty and its expected revision.
- Promote Alacritty-only scenarios to unconditional coverage. In particular,
  `check-quake.ts` currently gives Alacritty 40 animation cases versus one for
  Ghostty, and `check-fullscreen.ts` has Alacritty-only checks. Preserve the full
  scenario set, frame probes, lifecycle checks, and platform-specific assertions.
- Consolidate `bench:*:ghostty` tasks into the ordinary tasks and update CI
  callers. Preserve scroll, queue, link, and renderer gates and measurement
  meaning. Do not weaken thresholds or claim unmeasured performance gains.
- Update Linux package smoke evidence producers and `package-linux.ts` together;
  it currently reads one memory-map file per engine. Keep private-library,
  static-library, and host-Vulkan checks intact.

### Dependencies and documentation

Remove direct Alacritty dependencies and regenerate `Cargo.lock` without
unrelated upgrades. Inspect remaining dependency paths before deciding which
transitive crates should disappear. Update required-component inventories,
package verification, license/notices generation, schema editor fixtures, and
task descriptions to describe the shipped implementation accurately.

Preserve `scripts/ghostty-source.json`, vendor provenance, native source hashes,
build isolation, and reviewed fixes. The investigated native revision is
`22d13172cde98a0a4dda05d3d6a3fcb0dd8ed018`, with Zig `0.16.0` and wrapper/sys
version `0.2.1` incorporating upstream `5988a0b78b4aa804d1c12e66bbfe662bd97d81c0`.
Current manifests take precedence if the delivery base changes. A native upgrade
requires a demonstrated blocker and a separate decision.

Update README, development/configuration guidance, and AGENTS.md. Mark historical
plans with superseded engine decisions rather than rewriting recorded results.
Retain legitimate Alacritty theme attribution and third-party license provenance.

## Execution and evidence

1. Refresh and verify `origin/main`, repository access, and both review channels.
   Account for differences from the investigated baseline before preparing the
   feature branch. Settle the questions below and make the approved feature-owned
   plan available in a separate commit before implementer handoff.
2. Capture focused baseline Ghostty contracts, PTY/lifecycle tests, native input
   and clipboard behavior, and scroll/link budgets before changing behavior.
   Record existing failures separately. Exercise the additional desktop scenarios
   when promoting the currently asymmetric smokes; do not silently drop them if
   they expose a Ghostty failure.
3. Delegate implementation and focused verification to the assigned implementer.
   Keep verification close to each change. Require the handoff to account for
   every changed/untracked path, tests that actually ran, commands, results,
   unresolved failures, and environment limits.
4. Inspect the complete diff and evidence after ownership returns. Fill missing
   evidence, route confirmed corrections back to the implementer, and run
   `mise run license` and `mise run verify` before broad handoff. Complete the
   relevant platform and packaging verification below. Commit only feature-owned
   paths and compare unrelated local state before and after each commit.
5. Use `file-pr` to push and create a draft targeting `main`. Run CI and
   `dual-review` concurrently on the pushed candidate. Give both reviewers the
   same immutable base/head pair, contract, and revision-bound evidence ledger.
   The orchestrator does not count as an independent reviewer.
6. Reconcile both complete reports before acting. Batch confirmed findings into
   corrections and use `babysit-pr` through readiness. Reuse reviewers when valid;
   renew both perspectives for changes to architecture, public contracts,
   clipboard authority, lifecycle, concurrency, or supported-platform behavior.
   Keep the PR draft if either review channel or required evidence is incomplete.
7. Confirm the live final head, required CI, composed review coverage, resolved
   blockers, delivery checkout/branch/upstream, and preserved intake state before
   marking ready. Remove only safe workflow-created temporary resources. Hand
   back the PR URL, revisions, outcomes, gaps, and cleanup status. Leave merging
   to a separate instruction.

Keep a compact temporary evidence ledger with revision, command/observation,
scope, outcome, behavior proved, test names/counts, and environment limits.
Carry evidence forward only when ancestry and the intervening diff justify it.
Use `show-me-your-work` if material pivots or blockers need a separate decision
trail; do not commit that trail by default.

| Concern | Required evidence |
| --- | --- |
| Legacy migration and fallback | Focused config tests covering the table, warning presence/absence, unrelated invalid settings, preserved clipboard deny, and rejected reload retaining active settings. Schema byte check and editor smoke cover deprecated completion/validation. |
| Only one implementation and private native boundary | Compilation of all targets, dependency graph inspection, repository architecture gate, and launch-path review. No selector remains in runtime/protocol APIs. |
| Colors, rows, selection, links, and resize | Ghostty contract tests for dynamic defaults/palette, fragmented OSC, clean-row reuse, immutable older snapshots, generations, Unicode/soft wraps, stale selections, viewport anchoring, alternate screen, and bounded OSC 8 lookup. |
| Mouse and input | Contract tests retain exact current mode semantics and dequeue-time encoding; `smoke:macos-input` and `smoke:linux-input` validate default-engine native input. |
| Clipboard authority and delivery | Existing host-effect and clipboard tests plus native macOS/Linux clipboard smokes, including clean tmux, policy deny, revocation, recipient validation, bounded delivery, and hidden/background views. |
| Startup and lifecycle failure | Engine initialization failure launches no child; snapshot failures do not retry-loop; PTY tests preserve final output, exited history, rejected input, and cleanup. Default-engine native Quit/close coverage verifies cancellation and resumed shell input. |
| Desktop coverage survives consolidation | Relevant input, integration, palette, fullscreen, quake, clipboard, and macOS Quit smokes retain existing scenarios. Resize verifies grid/PTY restoration. Physical-device and unsupported-IME gaps remain explicit. |
| History and performance | Preserve the 16 MiB byte budget and actual retained-row reporting. Run ordinary scroll/link correctness and performance gates after consolidation and record renderer/engine measurements where affected. Respect Xvfb's frame-delivery limits. |
| Native builds and packages | Vendor/native integrity checks, license audit, and broad verification; verify macOS universal and Linux package inventories/linkage through applicable package tasks and CI. Preserve Sparkle-free normal packaging. Report native Intel UI coverage separately. |

New material tests should fail at their intended assertion before passing when
practical. Existing passing tests are regression evidence. Do not build a new
harness for mechanical deletion or prose; compilation, schema generation checks,
dependency inspection, and Markdown lint are proportionate there.

## Non-goals

Do not implement #77 or #82, change `TERM`/`COLORTERM`, introduce terminfo or SSH
integration, or enable additional Ghostty protocols. Keep presentation/query
authority separate from clipboard permission in future work. Do not change PTY
ownership, attachment semantics, retained-history policy, native pins, or build
protections as incidental cleanup.

## Delivery decisions and unresolved questions

The user approved the migration policy and warning presentation above and
selected CodeRabbit after Codex/Claude internal acceptance. Use its
provider-specific skill and route confirmed findings through the same correction
and verification process. There are no unresolved product questions at approval.
