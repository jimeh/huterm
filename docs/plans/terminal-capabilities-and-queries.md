# Terminal capabilities and presentation queries

## Outcome and delivery

Implement [#77](https://github.com/jimeh/huterm/issues/77) and
[#82](https://github.com/jimeh/huterm/issues/82) together. Applications should
discover Huterm's existing RGB support and receive color, appearance, and size
reports that agree with the terminal they see. The implementation base is
`46f26cf7c4a144ebdc90955ba9d122bb2e11364b`, after Ghostty-only PR #119.
References to Alacritty parity in the issues are obsolete.

Use `ship-feature-pr` through a ready PR, without merging or releasing. One
fresh `gpt-5.6-sol` worker at medium effort implements the sequential chunks
below. The orchestrator owns the plan, Git operations, complete diff
inspection, evidence sufficiency, and correction decisions. The worker has
exclusive write ownership during each chunk and leaves commits to the
orchestrator. Reuse its session for corrections and subsequent chunks.

Run one final independent Codex/Claude dual review after all implementation
chunks, against the pushed immutable head. Use `gpt-6-astra` at low effort for
Codex and the Claude review skill's configured model for Claude. Do not run
dual reviews between chunks. Run CodeRabbit once after internal acceptance, as
requested. Validate each chunk locally as it arrives. Corrections receive
proportionate verification and, where they invalidate final review coverage,
continuation by the affected original reviewers.

## Implementation contract

The private engine abstraction remains. Protocol types stay dependency-neutral;
core owns the emulator, PTY, query processing, and ordered writer. GPUI
publishes presentation inputs but never becomes a synchronous dependency of
parsing.

Publish retained presentation state rather than asking a client to answer every
escape sequence. Seed default colors, palette, appearance inputs, and known
cell geometry before spawning the child. A query before the first UI render
must have a defined answer. Runtime rows and columns remain authoritative.
Content pixel reports describe the grid, excluding window chrome and padding.

Use one explicitly authorized presentation controller per terminal. Reject
updates from detached or superseded controller generations. Reuse existing
attachment validation where appropriate, but do not tie presentation permission
to OSC 52 clipboard permission or focus-based clipboard recipient selection. Do
not implement general shared-view arbitration or IPC. Retain the last accepted
presentation after detach; a replacement controller publishes a coherent new
state. Values that were never available receive the native query's documented
no-answer behavior rather than invented geometry. Parsing never waits for a UI
response.

Theme reload updates every retained terminal, including inactive tabs. Font and
scale changes update physical cell dimensions even when grid dimensions do not
change. Serialize updates through the runtime control path and preserve its
existing queue bounds, retry behavior, and post-exit reply suppression.

Configure Ghostty's base colors and palette without overwriting application OSC
overrides. Derive appearance from the effective background with one documented,
tested luminance classification. Queries, rendering, resets, and theme reload
must agree. Supplying base colors must not turn semantic default cell colors
into permanent explicit RGB colors or bypass selection/theme behavior. Preserve
the existing palette override and damage tracking across input chunks.

Keep query parsing and ordered color mutations in Ghostty. Use the pinned
library's existing size and appearance callbacks. Do not add a parallel escape
parser, upgrade Ghostty, or require XTGETTCAP. Replies use the existing ordered
PTY writer. A color mutation, query, and second mutation in one input chunk
must report the color at the query point.

## Chunk 1: Runtime presentation state and authority

Inspect `huterm-protocol`, core terminal/runtime construction, attachment and
host-effect registration, and desktop tab spawning before choosing concrete
types. Add the smallest neutral state and update interface that satisfy the
contract. Separate initial launch state from later controller authorization so
the child cannot race initial publication.

Carry theme defaults, palette, and cell geometry to the engine wrapper.
Preserve runtime grid ownership and reject stale presentation updates. Define
behavior for missing initial geometry, controller replacement, detachment, and
terminal exit in API comments and tests. Avoid introducing a second resize
ordering path.

Evidence: focused tests for initial state, accepted and stale updates, retained
state after detach, clipboard-denied presentation updates, and bounded queue or
stopped-runtime behavior. Run the architecture gate. The orchestrator inspects
all paths and verifies that lifecycle and permission checks actually guard the
mutation, not just the caller.

## Chunk 2: Ghostty replies and desktop publication

Connect the state to Ghostty default foreground/background/cursor and palette
setters, its `on_size` callback, and its `on_color_scheme` callback. Support
OSC 4 and OSC 10/11/12 query behavior plus associated existing reset behavior
(OSC 104 and OSC 110/111/112). Support CSI 14t, 16t, and 18t reports and CSI
`?996n` appearance queries. Do not add side-effecting window operations.

Connect initial desktop theme publication, reload for hidden tabs, resize,
font, and display-scale updates. Use the existing grid metrics and terminal
layout. Audit snapshot conversion and row cache invalidation when supplying
base colors: default cells must remain responsive to theme changes, while
application RGB cells and OSC overrides retain their intended meaning.

Evidence: byte-exact engine and live-PTY fixtures for initial queries, indexed
and default colors, dark/light classification, mutations and resets, repeated
queries, every meaningful chunk split, malformed requests, theme changes
without row output, and ordered resize/query behavior. Check reply counts as
well as values. Exercise application startup before rendering. Include a native
smoke observation that reported content and cell pixels agree with the rendered
grid.

## Chunk 3: Terminfo identity and packaging

Audit a conservative `xterm-huterm` source entry against capabilities exposed
by Huterm. Retain indexed `setaf`/`setab` behavior and include the `Tc` flag used
by supported truecolor consumers; do not set ncurses `RGB`, which changes those
inherited capabilities to direct-color semantics. Do not copy `xterm-ghostty`
wholesale: the embedded VT library does not provide every Ghostty application
feature.

Package source and usable compiled terminfo for macOS bundles and Linux tarball
and AppImage layouts. Use existing resource discovery and packaging
conventions; account for relocatable packages and development launches. Select
`TERM=xterm-huterm` only when its entry can be found. Retain
`COLORTERM=truecolor` and `TERM_PROGRAM=Huterm`. Provide an explicit
compatibility override and a defined `xterm-256color` fallback. Preserve
relevant user terminfo search paths; do not install into machine-global
locations at application launch.

Document remote installation with `tic`, missing-entry symptoms, and a
per-command SSH compatibility fallback. Local discovery cannot establish remote
availability. Automatic SSH wrappers, remote installation, and Mosh protocol
changes are out of scope. Mosh color-query limitations must not be represented
as fixed by this PR.

Evidence: compile and inspect the entry, test discovery and fallback using
isolated directories, verify config validation and generated schema/defaults,
and test packaged resource paths. Run a private tmux server with empty
configuration and capture outer PTY bytes: representative `38;2;r;g;b` output
must remain RGB. Exercise a non-tmux terminfo consumer as well as explicit RGB
snapshot retention.

## Chunk 4: Integration, documentation, and delivery evidence

Integrate focused fixtures with discoverable Mise tasks and CI where required.
Keep subprocess tests isolated from the user's tmux server, shell
configuration, and installed terminfo. Clean up owned processes and temporary
files on failure. Document supported queries, authority/fallback rules,
terminal identity, packaging, and SSH/tmux behavior in the appropriate user and
architecture guides.

Run `mise run verify` and relevant Linux native smokes locally when available.
Run license checks if dependencies change. Use CI and available native macOS
execution for macOS build, packaging, and UI evidence; Linux results do not
prove AppKit geometry or packaging behavior. Report unavailable platform
observations as gaps instead of claiming success. Application checks should
cover Codex-style OSC 10/11 probing directly and through an ordinary SSH byte
transport; actual remote-machine access requires separate authorization.

Maintain a temporary revision-bound evidence ledger with command, revision,
scope, outcome, behavior proved, test names/counts, and environment
limitations. The worker reports evidence at each handoff. The orchestrator
inspects the full diff and fills gaps without repeating valid unaffected tests.
Use disposable checkouts for destructive validation; do not perturb a moving
implementation.

After the complete implementation passes proportionate local checks, create the
draft PR, run the single final dual review alongside CI, reconcile both
reports, and batch confirmed corrections. Mark ready only when the final remote
head has required checks and adequate composed review coverage. Keep the
invocation checkout as the final local feature home and preserve unrelated work
exactly.

## Acceptance and principal risks

| Requirement or risk | Required evidence |
| --- | --- |
| tmux silently quantizes RGB | Actual outer-PTY RGB bytes with empty tmux config |
| Missing local/remote terminfo | Isolated discovery/fallback tests and remote instructions |
| Query races initial UI render | Live-child query before first render |
| Theme reload overwrites OSC colors | Default/override/reset fixtures plus snapshot assertions |
| Clean rows hide a color-only change | Query and rendered-state assertions without text output |
| Pixel reports include chrome or stale scale | Grid metrics checks and native geometry smoke |
| Detached controllers change presentation | Generation/revocation tests and retained-state assertions |
| Clipboard denial suppresses unrelated queries | Independent-permission regression test |
| Replies duplicate or reorder | Exact byte/count tests and same-chunk mutation/query fixture |
| Malformed queries stall or grow work | Parser fixtures and existing bounded runtime paths |
| Packaging works only in source checkout | Relocated artifact resource verification on both platforms |

No unresolved product decisions block implementation. If source inspection
shows that this contract requires a native-library change, materially broader
authority model, or cannot support portable packaging, return that finding to
the orchestrator before expanding scope.
