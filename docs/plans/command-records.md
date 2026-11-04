# Command records and shell directories (#352)

Status: approved. Ships #352 only. Navigation, selection, and failure tint
stay with #38, command-finished notifications with #81, and restore with #42.

## Outcome

Each terminal's runtime records the commands its shell reports through OSC 133
and publishes them to every viewer as revisioned status. A record knows when
its command started and finished, how it ended, and the command line when the
shell sent one. Records keep a stable anchor in Ghostty's history, so they
survive reflow and leave when their rows leave history. Core also remembers the
root shell's own last local directory, so a new tab or split started while
`ssh` reports a remote directory opens in the local shell's directory instead
of the default.

## Decisions

Jim settled these on 2026-11-04.

- **Command text** is kept only when the shell sends it (OSC 133 C with
  `cmdline` or `cmdline_url`, decoded by Ghostty). Huterm's bash, zsh, and
  elvish scripts never send it; fish does. Core caps it at 4 KiB on a
  character boundary, drops control characters (C0, DEL, C1), stores an empty
  result as `None`, and never interprets it. Reading typed input from the grid
  is left to #81 if it wants text for every shell.
  The pinned parser captures OSC 133 in a fixed 2,048-byte buffer
  (`osc.zig` `MAX_BUF`) and rejects the whole sequence on overflow, so a C
  whose encoded command line is longer produces no event and no record, and
  its D is then ignored. The 4 KiB cap stays as a guard against a future pin.
  Preserving the lifecycle of such a C would need a native patch; that is a
  non-goal.
- **Bound:** a terminal keeps the 256 newest records, each holding one Ghostty
  tracked grid reference. Ghostty scans every tracked reference for each row
  it reflows. A C probe against the pinned `libghostty-vt` measured one width
  change at 100,000 rows of history: 4.8 ms best with no references, 9.1 ms
  with 256, 31 ms with 1,024, and 266 ms with 4,096. At 10,000 rows, 256
  references added about 1.5 ms. Older blocks keep Ghostty's own prompt row
  marks, which #38 can navigate, but lose their status.
- **Directory:** new tabs and splits keep inheriting the effective local
  directory (`terminal.new_tab_directory = "inherit"`). When that directory is
  remote, missing, or unusable, they fall back to the root shell's last local
  report before the normal launch directory. New windows still do not inherit.
- **No UI** in this pull request. No UX review is needed.

## Record model

Protocol (`huterm-protocol`, dependency-neutral, std types only):

```rust
pub const COMMAND_RECORD_CAPACITY: usize = 256;

pub struct CommandRecord {
    pub sequence: u64,               // 1-based per terminal, never reused
    pub text: Option<String>,        // shell-sent, bounded, untrusted
    pub state: CommandState,
    pub started_at: SystemTime,      // when C was handled
    pub finished_at: Option<SystemTime>,
    pub duration: Option<Duration>,  // monotonic clock, C to D or abandonment
}

pub enum CommandState {
    Running,
    Finished { exit_code: Option<i32> }, // D, with or without a status
    Abandoned,                           // ended without a D
}
```

- `TerminalStatus` gains `commands: Arc<[CommandRecord]>`, oldest first, so
  cloning status for each publication stays cheap.
- A record changes at most once after creation: from `Running` to `Finished`
  or `Abandoned`. Every other field is fixed, except `finished_at` and
  `duration`, which are set with that change. Only the newest record, or the
  record a new C supersedes, can change.
- Helpers: `command(sequence) -> Option<&CommandRecord>` and
  `commands_after(seen) -> (&[CommandRecord], u64)`, which returns records
  created after `seen` and how many of those the list no longer holds. A
  consumer that saw a record `Running` looks it up by sequence to observe its
  change. Names may follow local conventions; the shape is the contract.
- Records carry no screen position. A row offset changes with every output
  line, so publishing positions would wake every viewer on every chunk. #38
  adds a position query against core's anchors.
- `TerminalMetadata` gains `shell_directory()`: the root shell's last local
  OSC 7 report.

## Marker rules

Ghostty reports semantic prompt events through
`GHOSTTY_TERMINAL_OPT_SEMANTIC_PROMPT`: prompt start (A, with a prompt kind),
input start (B), output start (C, with command text), and command end (D, with
an optional exit code). Ghostty parses and bounds the OSC. It never reports a
sequence it rejects, but it accepts a marker whose options are malformed and
ignores those options; a D with an unparseable status arrives without an exit
code. Core handles each event in output order:

- **A (primary prompt kind only)** sets the pending prompt anchor at the
  cursor, replacing any earlier pending anchor. A repeated A, from a readline
  redraw, `zle reset-prompt`, or Ctrl-L, only moves the pending anchor. Right,
  continuation, and secondary prompt kinds are ignored. If a record is
  `Running`, it becomes `Abandoned`.
- **B** is ignored for records. Ghostty's cell semantics already mark input.
- **C** creates a record. Its anchor is the pending prompt anchor when one
  exists and still has a value, otherwise a new anchor at the cursor. (A RIS
  between A and C leaves the pending anchor without a value.) It clears the
  pending anchor. If the previous record is still `Running`, it becomes
  `Abandoned` first.
- **D** finishes the newest record only when it is `Running`. A D with no
  running record, such as the D a full readline redraw resends, is ignored.
- **Alternate screen:** A and C that arrive while the alternate screen is
  active are ignored. D still finishes a running record, because a crashed
  full-screen program can leave the alternate screen active when the shell
  reports. The screen is the one active when the marker was parsed, not when
  effects drain; see "Capturing event-time state".
- **End of output:** the runtime observes the root's exit before it reads
  output already queued, so exit alone must not settle records. Markers in
  output read after the exit apply normally. Once the root has exited and the
  reader reports end of output (`PtyEof` or a reader failure), a `Running`
  record becomes `Abandoned`. While a descendant still holds the PTY open, the
  record stays `Running`, because output, including a D, can still arrive.
- **Nested shells** use the flat model above. An outer `bash` or `ssh` record
  ends `Abandoned` at the inner shell's first prompt, and the outer D is then
  ignored. Huterm's five shells all send D before A, so a normal prompt never
  abandons the record it just finished.
- An exit code of 130 is just an exit code. Core does not infer signals.
- Markers annotate display state only. Nothing in a record is executed or
  interpreted, and explicit notifications (OSC 9, OSC 777) never create
  records; Huterm does not install those callbacks.

## Anchors, bounds, and eviction

- `huterm-ghostty` binds the semantic prompt callback as a new `Effect`
  (kind, prompt kind, exit code, copied text) and wraps tracked grid
  references in a safe owned type that frees on drop and stays valid after
  the terminal is freed (per `grid_ref_tracked.h`). It also binds
  `ghostty_terminal_vt_write_until`. Add the functions to the bindgen
  allowlist and regenerate with `mise run ghostty:bindings`. Keep unsafe code
  in the files the engine guide lists, with `// SAFETY:` comments citing the
  header. Callbacks stay read-only; they never create references.

### Capturing event-time state

Effects drain after the whole write, by which time output may have moved the
cursor, scrolled, or switched screens. Core therefore writes each chunk with
`vt_write_until` and stops at `OSC_END`. At each stop where the queue gained a
semantic prompt effect, core reads the cursor and the active screen and
creates the anchor there, before writing the rest of the chunk. Every byte is
still consumed in order, so terminal state matches a plain write.

- A `huterm-ghostty` contract test pins that the semantic prompt effect is
  queued by the `OSC_END` stop for both BEL and ST terminators. Ghostty stops
  an ST-terminated string after the ESC, before the `\`. If dispatch happens
  only on the `\`, also stop at `GROUND` after an `OSC_END` stop and capture
  there.
- The other effects (PTY replies, title, OSC 7, bell) keep their order. They
  may drain once at the end of the chunk as today.
- `bench:engine` throughput must not regress materially. Compare the
  existing fixtures before and after, including OSC-heavy output.

### Bounds

- At most 256 records hold references, plus one pending prompt anchor. Each
  marker is handled before the next byte is parsed, so no queue of anchors
  builds up within a write: a chunk of thousands of markers never holds more
  than 257 live references. A test with a dense marker chunk asserts the peak
  through a test-only live-reference counter.
- Creating record 257 drops the oldest record and frees its reference.
- **Eviction** removes records whose reference has no value. Command order
  need not match row order (cursor addressing can put a later command above an
  earlier one), so scan the whole list, at most 256 checks. Run it after each
  engine operation that can drop history: output, resize, scrollback limit
  change, clear scrollback, and reset.
  - The newest record is never removed by eviction, so the latest command's
    outcome stays observable even with `scrollback = 0`.
  - A record is never removed in the same publication in which it left
    `Running`. The next pass can remove it. A viewer therefore always sees a
    final state once, unless the count cap drops records created and finished
    within one chunk; the sequence gap reports those.
  - A record whose reference could not be allocated has no anchor. It stays
    published, eviction skips it rather than treating it as evicted, and it
    leaves through the count cap.
- **What anchors mean:** a reference follows Ghostty's tracked-pin rules.
  Scrolling, pruning, and reflow move it with its cell. RIS and history
  clears (ED 3, Huterm's clear scrollback, page pruning) remove its value, so
  the record leaves at the next pass. Erasing or deleting rows in place within
  the active area (ED 2 by a program not at a prompt, delete line, scroll
  regions) keeps the value and can leave it on replacement text. That is
  accepted. #38 validates a block against Ghostty's cell semantics before it
  uses one. Ctrl-L at a prompt scrolls the screen into history first
  (`Terminal.eraseDisplay`), so it keeps anchors on their text.
- Any change to the record list or a record's state publishes status once per
  output chunk, not once per event.

## Shell directory

- `foreground::Reports` records a directory report as the shell's when the
  foreground group at report time is the root shell's group (the child PID)
  and the report is local. An empty OSC 7 from the root shell clears it.
  Reports from other groups, such as a remote shell under `ssh`, never change
  it.
- `PublishedMetadata` includes it, so a change publishes status.
- GPUI's `inherited_directory` returns the candidates in order (effective local
  directory, then shell directory), and the spawn task uses the first one
  `usable_launch_directory` accepts. Tabs and splits share the helper.
- Update `docs/configuration.md` ("New tabs and panes start in ...") to
  describe the fallback.

## Constraints

- No second PTY parser: OSC 133 parsing stays in `libghostty-vt`.
- `huterm-protocol` stays dependency-free (`mise run architecture`).
- One runtime thread mutates each terminal; anchors live on it and never cross
  threads.
- The `bench:engine` `resize-history` gate (45 ms p50 at 100,000 rows) must
  hold with a full record list. Fill that fixture's history with OSC 133
  marks so it gates the realistic case, and report its numbers before and
  after.

## Non-goals

- Positions in protocol, navigation, selection, failure tint (#38).
- Notifications, and reading command text from the grid (#81).
- Saving or restoring records or directories (#42); records are not
  persisted.
- New windows inheriting a directory.
- Nested-shell stacks, OSC 133 `aid` and `redraw` options, which the pinned
  callback does not expose.
- Ghostty native patches: cheaper reflow with many references, or keeping a
  C whose command line exceeds the parser's 2,048-byte capture.

## Verification

| Concern | Evidence |
| --- | --- |
| Callback binding, kinds, exit code, decoded text, malformed options | `huterm-ghostty` contract test |
| `vt_write_until` stop queues the semantic effect for BEL and ST | `huterm-ghostty` contract test |
| Tracked references survive reflow and lose value on RIS and clears | `huterm-ghostty` contract test; `test:ghostty-releasesafe` |
| Event-time screen: enter alternate, C, leave alternate in one chunk | core engine test |
| Dense markers in one chunk keep live references at or below 257 | core engine test with a test-only counter |
| C at the 2,048-byte native limit and beyond | core engine test: record at the limit, none beyond, output intact |
| Pending A invalidated by RIS before C | core engine test |
| Chunk-split A/C/D, byte by byte | core engine test |
| Malformed and oversized OSC 133, text cap, control stripping; output unchanged | core engine tests comparing cells with and without the marks |
| Redraw: repeated A, D without C, Ctrl-L | core engine tests |
| Nonzero exit, D without code, abandonment by A and C | core engine tests |
| Root exit observed before queued D, before queued C and D, before queued C without D; abandonment at end of output | core runtime tests with deterministic ordering, no sleeps |
| A viewer sees C and D in separate publications for the same sequence | core runtime test |
| Alternate screen ignores A and C, accepts D | core engine test |
| Offscreen and metadata-only markers publish status without a snapshot or changed rows | core runtime test on a hidden viewer |
| Reflow keeps anchors on the same text | core test with a test-only anchor accessor reading the anchored row's text, narrowing and widening |
| Eviction drops invalid records anywhere in the list, keeps the newest and records that just finished; clear scrollback; cap at 256; command order differing from row order | core engine tests |
| OSC 7 remote host, change, chunk split | existing directory tests, extended as needed |
| Shell directory: root report, `ssh`-style remote report from another group, clear | `foreground` unit tests and a core runtime test |
| Inheritance falls back to the shell directory | GPUI unit test on the helper |
| Reflow cost with 256 records | `bench:engine` `resize-history` gate |
| Native: bash (Apple 3.2 and 5.x), zsh, fish, nushell, and elvish produce records for normal, failing, multiline, and full-screen commands; prompt redraw makes no extra record; `cd` updates both directories | PTY integration tests beside `shell_integration/native_tests.rs` that assert record sequence, state, and exit code, not only transcript marks; `HUTERM_REQUIRE_SHELLS` rules apply; minimal allowlisted environment, no inherited values in logs |
| Directory inheritance in the app | existing desktop tests plus the helper test; no new smoke unless a gap shows |
| Linux | CI Checks and Xvfb smoke |

## Open questions

None.
