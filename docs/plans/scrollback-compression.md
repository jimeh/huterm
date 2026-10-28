# Scrollback compression

Status: implemented. Approved on 2026-10-27, when Jim accepted every
recommendation in [Open questions](#open-questions). PR 1 (#283) added the
bindings, safe API, tests and benchmarks; PR 2 added the scheduler and
setting. See [Implementation outcome](#implementation-outcome).

Ghostty 1.4.0 can compress scrollback that is off screen. It encodes each cold
history page with LZ4, keeps the page's virtual mapping, and returns its
physical memory to the operating system. Any read of the page restores it.
The Ghostty application schedules this itself; `libghostty-vt` leaves
scheduling to the embedder through `ghostty_terminal_compress` and an activity
token. Huterm has never called either, so every terminal holds its whole
history resident: about 10 MB at the default 10,000 rows and about 100 MB at
the 100,000-row maximum.

This plan records prototype measurements, answers whether Huterm should
compress, and describes how. It recommends enabling compression by default
behind a `[terminal] scrollback_compression` setting, scheduled on each
terminal's runtime thread once neither the terminal nor its user has done
anything for 250 ms. The main cost is that the first width change after a
pass restores all history, taking two to about three times today's reflow
time.

## What Ghostty provides

Read from the pinned source under `.native/ghostty/source`:

- `ghostty_terminal_compress(terminal, mode, &result)` in
  `include/ghostty/vt/terminal.h`. Incremental mode inspects at most eight
  cold pages and compresses at most one per call
  (`incremental_compression_max_inspected` in `src/terminal/PageList.zig`),
  then returns `PENDING` or `COMPLETE`. Every pass ends with a verification
  traversal, and a pass completes only when that traversal compresses
  nothing, so pages restored between steps are recompressed. Full mode
  compresses every eligible page in one call. `UNSUPPORTED` means the target
  cannot discard physical memory while keeping the mapping.
- Eligible pages are complete history pages before the active area that do
  not intersect the viewport (`CompressionIterator`). Only the primary screen
  is compressed (`Terminal.compress`); the alternate screen has no history.
- A step compresses one whole page, and pages are not a fixed size. They grow
  to hold styles, graphemes and hyperlink strings (`increaseCapacity`), up to
  `size.max_page_size`, which is `maxInt(u32)`. A step's cost therefore has
  no fixed upper bound.
- `ghostty_terminal_compression_activity` returns an opaque token. PageList
  operations that may create compression work change it: `grow`, `scroll`,
  `resize`, `compact`, `split`, `increaseCapacity`, erasing rows and limit
  enforcement (`page_compression.markActivity`). Restoring a page does not.
  The token covers only the primary screen's pages, so programs on the
  alternate screen never change it.
- `GHOSTTY_TERMINAL_DATA_MEMORY_USAGE` fills `GhosttyTerminalMemoryUsage`:
  `compression_supported`, then pages, virtual, resident, compressed pages and
  compressed bytes for each screen. It reads page metadata and never restores
  a page, but visits every page.
- `Node.page()` restores a compressed page in place and leaves it resident.
  `pagePreservingState()` decodes into a caller-owned copy and leaves the node
  compressed. Ghostty's search, snapshot encoder and inspector use the latter;
  Huterm calls none of them. The paths Huterm does call, render-state updates,
  grid references and selection formatting, restore through `page()`.
  Huterm's `page-list-pin-nearest` patch walks node metadata only, so
  resolving a reference costs nothing extra; reading its cells restores the
  one page that holds them.
- `PageList.resize` resets compression state with the comment "Resizing forces
  all nodes to be decompressed today". The upstream TODO is deferred reflow of
  off-screen pages.
- `Screen.scrollClear` moves the written screen into history one row at a
  time, which can recycle the oldest page, then re-registers the cursor's
  style and hyperlink on its new page. ED 22 calls it, and so does ED 2 when
  the last non-empty row is a shell-integration prompt, as Ctrl-L at a prompt
  sends.
- `mem.canReclaim(.strict)` is true on 64-bit Linux (`MADV_DONTNEED`), Darwin
  (`MADV_FREE_REUSABLE`), Windows and FreeBSD, and false on 32-bit targets
  and everything else. Darwin returns memory lazily, so a process's RSS stays
  high until the system needs the memory; the physical footprint drops at
  once. In test builds `canReclaim` is always true and decommit only zeroes
  the range, so Ghostty's own compression tests never reach `madvise`.
- Compressed copies are allocated through the terminal's allocator. Huterm
  passes none (`Terminal::new` calls `new_in` with `None`), so they come from
  libc `malloc`.
- None of these calls is thread-safe with other access to the terminal.
- The Ghostty application compresses by default (`scrollback-compression =
  true` in `src/config/Config.zig`). Its renderer thread restarts a 250 ms
  idle timer when the token changes, then runs one incremental step every
  1 ms until the pass completes (`Compression` in `src/renderer/Thread.zig`).
  Turning the setting on clears its remembered token, so existing history is
  reconsidered.
- The snapshot decoder option `GHOSTTY_SNAPSHOT_DECODER_OPT_COMPRESS_HISTORY`
  compresses pages as a snapshot is restored. Huterm does not restore
  snapshots, so it does not apply.

## Method

A throwaway prototype on a local scratch branch, never pushed:

- The bindgen allowlist gains `ghostty_terminal_compress`,
  `ghostty_terminal_compression_activity`, `GhosttyTerminalMemoryUsage` and
  the `MEMORY_USAGE` data key. `huterm-ghostty` gains three methods on
  `Terminal`, and `huterm-core`'s engine gains test-only accessors.
- `crates/huterm-core/src/engine/compression_probe.rs` holds ignored release
  tests, each run in its own process. Terminals are 120 by 40 with a
  100,000-row limit and saturated history, as in `bench:engine`'s
  `resize-history`. A probe drains compression by calling incremental mode
  until it stops returning `PENDING`, timing every call.
- Synthetic fixtures, each about 121,000 rows of output from a seeded
  generator:
  - `plain-149col`: the `resize-history` line, 149 columns of mostly `x`.
  - `build-log`: Cargo-style output with SGR-colored `Compiling`, warnings,
    source excerpts and test results.
  - `ls-lR`: `ls -lR`-style listings with directory headers.
  - `colored`: a 256-color foreground and truecolor background on every word.
  - `cjk`: CJK, Hangul, half-width katakana, combining marks and emoji mixed
    with ASCII words.
  - `random`: random printable ASCII, the incompressible worst case.
  - Three fixtures that grow pages: four OSC 8 links with unique 200-byte
    URIs per line (`osc8-unique`), three combining marks on every cell
    (`graphemes`), and a distinct truecolor pair on every cell
    (`styles-per-cell`).

  The text fixtures draw words from a 27-word vocabulary, which flatters
  their ratio, so three captured outputs were also replayed until history
  saturated: `ls -lR` of a Cargo registry (117,916 lines, replayed twice),
  `git log -p --color=always` of this repository (200,000 lines), and the
  last 184,990 lines of macOS's `/var/log/install.log`.
- Ghostty's resident figure is `primary_resident_bytes`. Process memory is
  the physical footprint (`ri_phys_footprint` from `proc_pid_rusage`) on
  macOS and `VmRSS` on Linux. Resize peaks are a footprint sample taken every
  20 µs on another thread on macOS, because `ri_interval_max_phys_footprint`
  could not be reset; on Linux they are `VmHWM` after writing `5` to
  `/proc/self/clear_refs`. The process peak that #270 gates is
  `getrusage` `ru_maxrss` on macOS.
- Every operation timed on a compressed engine was also timed on an
  uncompressed twin with identical content. The twin runs first, so it pays
  any cold-cache cost; an earlier run in the other order showed the
  compressed engine paying it instead.
- Correctness probes compare a compressed engine with an uncompressed twin:
  the full extracted text, then complete snapshot rows (cells, styles,
  colors, wide flags and graphemes), the cursor, and link lookups at three
  points, at four scroll positions from the live bottom to the top.

Measurements were taken with Ghostty 1.4.0 (`83fb95624b`), on the bump's
pre-merge head `25290bbf`. The merged bump, #280, differs from that head
only in tests, documentation and the rebase, so the measurements apply to
it. They ran on an Apple M3 Max running macOS 27,
and for the first round also in the Linux arm64 Docker image
(`mise run linux:exec`, Ubuntu 22.04) on the same host. The second round,
which added the replayed, grown and per-fixture resize cases, ran on macOS
only. Elapsed times include scheduler preemption. A first-round run on the
previous pin, `cec8b92f`, gave the same sizes and step times within noise.

## Measurements

### Savings at 100,000 rows

"Resident" is Ghostty's `primary_resident_bytes`. "Footprint" is the growth
of the process's physical footprint over its value before the terminal was
created. A pass is every incremental step until `COMPLETE`.

| macOS fixture | Resident before | Resident after | Saved | Footprint after fill | Footprint after pass |
| --- | --- | --- | --- | --- | --- |
| plain-149col | 98.8 MiB | 1.6 MiB | 98.4% | 97.7 MiB | 3.6 MiB |
| build-log | 98.8 MiB | 10.9 MiB | 89.0% | 97.8 MiB | 13.3 MiB |
| ls-lR | 98.8 MiB | 11.2 MiB | 88.6% | 97.8 MiB | 13.2 MiB |
| colored | 162.1 MiB | 79.2 MiB | 51.1% | 152.3 MiB | 80.9 MiB |
| cjk | 98.8 MiB | 12.8 MiB | 87.0% | 100.9 MiB | 18.5 MiB |
| random | 98.8 MiB | 34.9 MiB | 64.7% | 97.6 MiB | 37.9 MiB |
| Cargo registry `ls -lR` | 98.4 MiB | 4.6 MiB | 95.3% | 99.3 MiB | 6.3 MiB |
| `git log -p --color` | 98.4 MiB | 7.5 MiB | 92.4% | 99.4 MiB | 9.4 MiB |
| `install.log` | 98.4 MiB | 7.8 MiB | 92.0% | 99.3 MiB | 10.1 MiB |

Every fixture compresses all pages but the one at the active boundary,
except `cjk`, which leaves two. On macOS the process RSS grows instead of
shrinking, by more than the encoded size (4.5 MiB against 1.2 MiB encoded
for `plain-149col`), because Darwin reclaims `MADV_FREE_REUSABLE` memory only
under pressure.

On Linux arm64 the synthetic fixtures save the same fractions, and `VmRSS`
falls at once because `MADV_DONTNEED` discards the pages. Linux pages are
slightly smaller, so the same history uses 258 or 259 pages:

| Linux arm64 fixture | Resident before | Resident after | Saved | VmRSS after fill | VmRSS after pass |
| --- | --- | --- | --- | --- | --- |
| plain-149col | 98.8 MiB | 1.6 MiB | 98.4% | 94.5 MiB | 3.4 MiB |
| build-log | 98.8 MiB | 10.8 MiB | 89.0% | 94.8 MiB | 13.0 MiB |
| ls-lR | 99.1 MiB | 11.2 MiB | 88.7% | 94.9 MiB | 13.1 MiB |
| colored | 161.3 MiB | 79.0 MiB | 51.0% | 153.1 MiB | 81.1 MiB |
| cjk | 99.1 MiB | 12.5 MiB | 87.4% | 100.7 MiB | 14.3 MiB |
| random | 98.8 MiB | 34.8 MiB | 64.8% | 94.5 MiB | 36.7 MiB |

The page-growing fixtures reach Huterm's derived 4 KiB-per-row byte
allowance before the row limit, so `graphemes` and `styles-per-cell` hold
about 64,000 and 67,000 rows:

| macOS fixture | Average page | Resident before | Resident after | Saved |
| --- | --- | --- | --- | --- |
| osc8-unique | 1.2 MiB | 304.1 MiB | 48.6 MiB | 84.0% |
| graphemes | 2.5 MiB | 394.9 MiB | 81.7 MiB | 79.3% |
| styles-per-cell | 2.3 MiB | 396.1 MiB | 173.9 MiB | 56.1% |

### Step cost

| macOS fixture | Steps | Step p50 | Step p99 | Step max | Pass |
| --- | --- | --- | --- | --- | --- |
| plain-149col | 284 | 57 µs | 79 µs | 122 µs | 13 ms |
| build-log | 284 | 192 µs | 234 µs | 248 µs | 49 ms |
| ls-lR | 284 | 173 µs | 227 µs | 299 µs | 45 ms |
| colored | 284 | 1.16 ms | 1.39 ms | 1.44 ms | 299 ms |
| cjk | 283 | 217 µs | 311 µs | 931 µs | 57 ms |
| random | 284 | 450 µs | 855 µs | 2.09 ms | 122 ms |
| Cargo registry `ls -lR` | 283 | 106 µs | 143 µs | 151 µs | 27 ms |
| `git log -p --color` | 283 | 156 µs | 333 µs | 360 µs | 43 ms |
| `install.log` | 283 | 165 µs | 289 µs | 417 µs | 43 ms |
| osc8-unique | 284 | 1.02 ms | 1.13 ms | 1.19 ms | 260 ms |
| graphemes | 180 | 2.28 ms | 2.62 ms | 3.11 ms | 369 ms |
| styles-per-cell | 189 | 3.82 ms | 4.08 ms | 4.14 ms | 646 ms |

On Linux arm64 the step medians are within about 10% of macOS: from 50 µs
(plain) to 1.08 ms (colored), with passes from 13 ms to 281 ms. Maxima vary
more; the longest Linux step was 2.57 ms (colored), and build-log's was
443 µs against 248 µs on macOS. Full mode compresses the plain fixture in one
10 ms call; the colored one would take about 300 ms in one call.

### Resize

Alternating between 100 and 120 columns at 40 rows. The compressed runs drain
a pass before every resize, the worst case. #270's gates apply to the
uncompressed `plain-149col` fixture only: a 45,000 µs median and a
143,360 KiB (140 MiB) process peak.

| macOS, 100,000 rows | plain-149col | build-log | colored |
| --- | --- | --- | --- |
| Resize p50, uncompressed | 7.3 ms | 10.0 ms | 57.2 ms |
| Resize p50, compressed | 14.3 ms | 31.9 ms | 108.5 ms |
| Resize max of 20, compressed | 20.4 ms | 35.6 ms | 140.4 ms |
| First resize, uncompressed / compressed | 7 / 21 ms | 10 / 32 ms | 95 / 207 ms |
| Peak footprint during the first resize, uncompressed | 99.5 MiB | 99.7 MiB | 222.5 MiB |
| Peak footprint during the first resize, compressed | 88.0 MiB | 97.5 MiB | 300.8 MiB |
| Process `ru_maxrss`, uncompressed | 105.0 MiB | 105.2 MiB | 224.8 MiB |
| Process `ru_maxrss`, compressed | 107.9 MiB | 136.4 MiB | 303.2 MiB |
| Pass after a resize | 14 ms | 47 ms | 153 ms |

Every width change leaves no page compressed. A height-only resize took 3 µs
and left every compressed page compressed in all three fixtures.

At the default 10,000 rows, the build-log median is 2.8 ms compressed against
1.1 ms, and the colored one 10.3 ms against 5.0 ms. On Linux arm64 at
100,000 rows, the plain medians are 13.6 ms compressed against 6.8 ms, and
`VmHWM` during the first resize, reset just before it, is 89.8 MiB against
98.5 MiB.

### First access to compressed history

The build-log fixture, compressed, against an uncompressed twin with the same
content, on macOS. Times include the snapshot that follows each scroll.

| Operation | Compressed | Uncompressed | Pages restored |
| --- | --- | --- | --- |
| Snapshot at the live bottom | 2 µs | 6 µs | 0 |
| Scroll up one screen, three times | 137, 124, 125 µs | 158, 125, 126 µs | 0 |
| Scroll up one row, 200 times | 27.5 ms | 27.2 ms | 1 |
| Scroll up 1,000 rows | 223 µs | 162 µs | 1 |
| Scroll to the top | 226 µs | 138 µs | 1 |
| Scroll to 50,000 rows back | 286 µs | 154 µs | 1 |
| Pass after returning to the live bottom | under 1 ms, 66 steps | | |
| Copy 10 rows mid-history | 84 µs | 47 µs | 1 |
| Copy all history | 43.4 ms | 23.4 ms | 252, all |
| 200 three-line outputs with snapshots | 22.8 ms | 22.3 ms | 2 recycled |
| 20,000 lines of output | 3.17 ms | 2.36 ms | 50 recycled |
| Clear Scrollback | 5.0 ms | 2.3 ms | 252 freed |

On Linux arm64, recycling under the 20,000-line burst cost nothing extra
(4.9 ms against 5.1 ms) and Clear Scrollback was faster compressed (0.2 ms
against 2.5 ms). The macOS penalties probably come from Darwin's
`MADV_FREE_REUSE` when a discarded mapping is reused; that is an inference,
not a measurement.

Link lookup, using `bench:links`' history fixtures at 100,000 rows, restores
one page in `scan-limit-above-viewport` and `scan-limit-mid-history`, which
take 254 and 262 µs on their first call against 242 and 241 µs uncompressed.
The other five fixtures restore nothing and are no slower.

### Activity token

After a completed pass on saturated build-log history, the token changed after
one new line, 2,000 lines, scrolling up one row, and scrolling back to the
live bottom. It did not change after a snapshot, a memory query, an in-place
redraw, a title change, or a selection extraction that restored a page. A
pass with nothing to compress takes 64 steps and 13 to 16 µs in total.

### Correctness and safety checks

The comparison against an uncompressed twin found no difference for any of
the nine synthetic fixtures after compression, after width changes in both
directions, across an alternate-screen round trip with a pass run while the
alternate screen was active, after Clear Scrollback, and after reset. A
separate probe ran ED 22 and ED 2 at a shell-integration prompt three times
each over compressed, saturated history with a bold red cursor style and an
open OSC 8 hyperlink, then lowered the scrollback limit from 20,000 to 5,000
rows; snapshots, text and links matched after every step. Both clears took
Ghostty's `scrollClear` path: history grew by 39 and 40 rows on the first
clear, and the compressed pages stayed compressed. While the alternate screen
was active, a pass still compressed primary history and left the alternate
page alone.

The probes compare full text before snapshots, and extracting the full text
restores every page, so the snapshot and link comparisons read pages that
selection restored rather than pages `RenderState` restored. The engine tests
in [Testing strategy](#testing-strategy) compare snapshots first.

Against ReleaseSafe Ghostty, built with the flags `test:ghostty-releasesafe`
uses, these probes also passed with the same sizes: savings for the six
synthetic and three page-growing fixtures, the `git log` replay, first
access, link lookup, the activity token, the twin comparisons for all nine
fixtures, both clears, full mode, and the compressed plain resize. The
build-log and colored resizes did not run against ReleaseSafe. The first
round also passed on Linux arm64, where `compression_supported` is true.

## Findings

### 1. Resize and reflow

Yes. Any width change restores every compressed page before it reflows
(`PageList.resize`), and the cost depends on how much a page holds. Plain
history resizes in 14.3 ms instead of 7.3 ms at 100,000 rows on Apple
silicon, build-log history in 31.9 ms instead of 10.0 ms, and colored history
in 108.5 ms instead of 57.2 ms. Restoring costs about 28 µs per page for the
plain fixture and about 87 µs for build-log pages. At the default 10,000 rows
the extra cost is 1.7 ms for build-log history and 5.3 ms for colored.

At the maximum, a resize after a pass takes about two frames at 60 Hz for
ordinary logs. [Scrollback history scale](scrollback-history-scale.md) chose
100,000 rows to keep a reflow under half a frame on Apple silicon, measured
on plain history. Uncompressed build-log and colored history already miss
that 8.3 ms target, at 10.0 and 57.2 ms. Compression triples the build-log
reflow time, to about four times the target. CI's x86_64 runners reflow
about 1.8 to 4.5 times slower than Apple silicon today, so a compressed
build-log resize there would take roughly 60 to 140 ms.

Memory during the resize is not always lower than today:

- For plain and build-log history, the peak footprint during the first
  resize is at or below the uncompressed peak.
- For colored history it is 300.8 MiB against 222.5 MiB, and the footprint
  after the resize stays about 78 MiB above the uncompressed run. That is
  the size of the freed compressed copies, so `malloc` probably keeps them;
  this is an inference.
- The process `ru_maxrss` that #270 gates is higher with compression in
  every fixture, by 30% for build-log, because macOS RSS still counts
  reusable pages. The uncompressed gate itself is unaffected, but a
  compressed fixture needs its own process and budget.

The saving is gone until the next pass, which takes 14 to 153 ms of steps.
A height-only change keeps every page compressed. During a window drag only
the first resize pays, because a resize changes the token and no pass runs
until 250 ms after the last one.

### 2. First access

Reading compressed history restores only the pages read, at about 0.1 ms
each for build-log pages. Restoring a grown page decodes about six times as
many bytes, from an encoded copy up to 25 times larger, and was not timed on
first touch. One page holds about 400 rows at 120 columns. The first
three screens back restored nothing, because those rows sit on the page at
the active boundary, which a pass never compresses. Scrolling back 1,000 rows,
to the middle or to the top restores one page and adds 60 to 130 µs, and
scrolling one row at a time restores a page every 400 rows. Link
lookup restores at most the one page above a mid-history viewport and adds
about 10 to 20 µs. Copying a selection restores every page it covers: copying
all 100,000 rows takes 43 ms instead of 23 ms and leaves all history resident
until the next pass. Huterm has no history search yet; Ghostty's own search
reads compressed pages without restoring them, so adopting it later would not
undo compression.

`bench:scroll` does not need a compressed variant. Its cost is dominated by
snapshot construction, and the first touch of a build-log page adds about
0.1 ms. The compression benchmark measures first-touch cost directly,
including colored and grown pages; if those prove expensive, that changes
this answer.

### 3. Snapshots and other runtime paths

Nothing on the steady-state path undoes compression:

- `GhosttyEngine::snapshot` (`crates/huterm-core/src/engine/ghostty.rs`)
  calls `RenderState::update`, which reads only viewport pages. A pass never
  compresses those, so a snapshot at the live bottom restored nothing. After
  a scroll moves the viewport onto compressed history, the next update
  restores the one or two pages it shows.
- `viewport_state` and `viewport_offset` read the scrollbar and history row
  counts, which come from page metadata.
- `link_reader` resolves rows through `RowRef` and the
  `page-list-pin-nearest` patch, which walks node metadata; only the cells
  read restore their page.
- `extract_text` calls `format_plain`, which restores every page in the
  selection.
- Writing output restores nothing except the oldest page when saturated
  history recycles it. Recycling discards the encoded copy without decoding
  it. On macOS that made a 20,000-line burst 0.8 ms slower until the
  compressed pages were used up; on Linux it cost nothing.
- `set_scrollback`, Clear Scrollback and reset free compressed pages without
  decoding them: `destroyNodeExt` restores with `.discard`, which skips the
  decode. `scrollClear` reads only the active area's pages, which are never
  compressed, and recycles the oldest page as it grows.
- The resize path restores everything, as finding 1 describes.

### 4. Scheduling

The runtime thread already owns every access to its terminal, so a step
between messages is serialized with writes, snapshots, selection and link
lookup without a lock. The idle point is the `Err(TryRecvError::Empty)` branch
of `run_terminal` in `crates/huterm-core/src/terminal.rs`, where the thread
otherwise waits on `wake.wait_until(probes.deadline())`. [Design](#design)
describes the hook.

The token alone is not a good enough definition of idle. It ignores redraws,
title updates and snapshots, so a full-screen program animating in place
lets a pass complete once and then costs nothing. But echoing a typed
character at a prompt is also an in-place redraw, so with the token alone a
pass would run while the user types, and every keystroke and echo could wait
for a step. Treating each client input message as activity removes that
overlap: a pass then starts only after 250 ms without a token change and
without input. Input only postpones a pending pass. It never starts one,
because typing at a prompt changes no history and would otherwise schedule a
64-step pass that finds nothing.

A step compresses one page, and its cost grows with the page. On Apple
silicon, text pages take 0.1 to 0.4 ms, colored and hyperlink-heavy pages
about 1.2 ms, and pages grown by graphemes or per-cell styles up to 4.1 ms.
Nothing bounds a page's size, so a pathological page could take longer.
Output that arrives during a step waits for that step and no more, because
the loop checks its queues between steps. With input counted as activity,
only the first keystroke after an idle period can meet a step in progress.
In-place output, such as a spinner, can arrive during every step of a pass.

The existing `bench:output-latency` workloads never build history or idle for
250 ms, so they never run during a pass. Measuring the cost needs a variant
that makes a long pass and types into it;
[Testing strategy](#testing-strategy) describes it.

### 5. Savings

Captured real output at the maximum drops from about 98 MiB to 5 to 8 MiB
per terminal, a 92% to 95% saving; the synthetic text fixtures save 87% to
89%, and plain repeated text 98%. Per-word truecolor saves 51%, random bytes
65%, and the page-growing fixtures 56% to 84%. The process footprint follows
Ghostty's resident figure within 2 to 6 MiB on macOS, and Linux `VmRSS`
within about 2 MiB. On macOS, RSS does not show the saving: Activity
Monitor's Memory column, which reports the footprint, does.

### 6. Configuration and diagnostics

A boolean setting, on by default, matches Ghostty's application and costs one
schema entry. The diagnostic should report whether compression is enabled,
disabled or unsupported. Memory statistics belong in `bench:engine`, where a
gate can catch a pin bump that stops compressing; no user-visible readout is
needed yet. See [Open questions](#open-questions).

### 7. Platforms

`mem.canReclaim(.strict)` decides support at compile time: true on 64-bit
Linux, Darwin, Windows and FreeBSD, false on 32-bit targets. Huterm ships a
universal macOS app (arm64 and x86_64) and Linux x86_64 and aarch64
packages, all 64-bit, so no shipped target reports `UNSUPPORTED`. macOS arm64
and Linux arm64 reported support in the probes, with the same savings.
x86_64 support follows from the same source lines, and CI's x86_64 runners
will exercise it through the new contract tests and benchmarks. The
scheduler still handles `UNSUPPORTED` by stopping, so a future target costs
one call.

### 8. Risks

- **Test coverage upstream.** The LZ4 codec has a differential property
  suite and PageList has 28 compression tests, but those tests simulate
  decommit by zeroing memory, so they never call `madvise` or recommit a
  real mapping. None of Ghostty's fuzzers (`fuzz_stream`, `fuzz_parser`,
  `fuzz_osc`) call compression. Huterm's tests link a release library and
  would exercise the real path; its ReleaseSafe job would run them with
  Zig's safety checks. A Huterm fuzzer is out of scope.
- **Failure mode.** A decode failure panics inside Ghostty (`failed to
  restore compressed terminal page`), and `libghostty-vt`'s release panic
  handler prints the message and traps (`panic` in `src/lib_vt.zig`). That
  ends the whole Huterm process and every terminal in it, as any other
  Ghostty panic does today. Compression adds new native code on a path that
  every terminal with scrolled-back history takes.
- **Resize.** Two to about three times the reflow time for the first width change
  after a pass, beyond the half-frame target at the maximum, and on macOS a
  higher transient footprint for style-heavy history.
- **Step size.** A step on a grown page took up to 4.1 ms and has no upper
  bound. Output and input wait for at most that one step.
- **Alternate screen, clear, reset and `scrollClear`.** Only primary history
  is compressed. Clear Scrollback, reset, `scrollClear` and limit changes
  free or recycle compressed pages without decoding them, and the twin
  comparisons found no difference after each.
- **Native surface.** Two functions, one struct and one data key, wrapped in
  about 100 lines that expose plain values. No handle leaves `huterm-ghostty`.
- **macOS RSS.** Tools that read RSS show no saving until the system needs
  memory, which may draw bug reports. The configuration documentation should
  name the footprint.

## Recommendation

Enable compression by default, behind `[terminal] scrollback_compression`,
with a 250 ms idle interval that client input postpones, and bounded,
spaced incremental steps on each terminal's runtime thread. Real output
shrinks by 92% to 95%, the runtime's single-owner thread makes serialization
free, and the costs fall on rare operations: a width change after an idle
period, copying a large selection, and a step that delays the first
keystroke after idle. At the default 10,000 rows those costs are a few
milliseconds. At the 100,000-row maximum a resize after a pass takes about
two frames for ordinary logs; open question 4 asks whether to accept that.

## Design

### `huterm-ghostty`

Add to the bindgen allowlist in `crates/huterm-ghostty/bindgen/src/main.rs`:
`ghostty_terminal_compress`, `ghostty_terminal_compression_activity`, the
`GhosttyTerminalMemoryUsage` type and the `MEMORY_USAGE` terminal data key,
then regenerate with `mise run ghostty:bindings`. The generated `keys.rs`
already marks the struct as sized, and `layout.rs` gains its ABI entries.

The safe API adds plain values only:

```rust
pub enum Compression { Pending, Complete, Unsupported }

pub struct ScreenMemory {
    pub pages: u64,
    pub virtual_bytes: u64,
    pub resident_bytes: u64,
    pub compressed_pages: u64,
    pub compressed_bytes: u64,
}

pub struct MemoryUsage {
    pub compression_supported: bool,
    pub primary: ScreenMemory,
    pub alternate: ScreenMemory,
}

/// Whether this target can compress, mirroring Ghostty's
/// `canReclaim(.strict)` for Huterm's targets.
pub const COMPRESSION_SUPPORTED: bool = cfg!(target_pointer_width = "64");

impl<H> Terminal<H> {
    /// One bounded incremental step.
    pub fn compress_step(&mut self) -> Result<Compression>;
    /// The opaque activity token; only equality is meaningful.
    pub fn compression_activity(&self) -> Result<u64>;
    pub fn memory_usage(&self) -> Result<MemoryUsage>;
}
```

`compress_step` takes `&mut self`, so the borrow checker serializes it with
writes, render updates and grid references, which already borrow the
terminal. An unknown result value maps to `Error::InvalidValue`, not to
`Unsupported`. Full mode stays out of the API: it is never right on an
interactive runtime thread. Image byte fields stay out because the adapter
disables Kitty graphics. A contract test asserts that
`COMPRESSION_SUPPORTED` equals `MemoryUsage::compression_supported`, so a
pin bump that changes platform support fails, and core re-exports the
constant for the diagnostic. The change is about 100 lines plus generated
code, and no native handle leaves the crate.

### `huterm-core`

`TerminalEngine` forwards the three calls. A new `CompressionSchedule`, beside
`ProbeSchedule` and built the same way from explicit `Instant`s so its tests
need no clock, holds:

- whether compression is enabled by configuration and still supported;
- the last activity token it saw, or none;
- a deadline, the instant the next batch may run.

The runtime thread drives it:

1. **Observe.** After it handles each output batch, client message, snapshot
   request, selection extraction, resize, edit or scrollback change, the
   runtime reads the activity token. A changed token restarts the deadline at
   now plus 250 ms, Ghostty's own idle interval. Selection extraction and
   link lookup restore pages without changing the token, so they start or
   restart it too. `RuntimeMessage::Input`, which carries keys, pastes and
   mouse reports, only postpones a deadline that is already set, so typing
   never starts a pass. Reading the token is a field load behind one FFI
   call.
2. **Wait.** The idle wait becomes
   `wake.wait_until(earliest(probes.deadline(), compression.deadline()))`.
   With no deadline, the thread sleeps as it does today, so a terminal whose
   pass has completed adds no wakeups.
3. **Step.** Compression runs only from the branch where `next_message` found
   no output and no client message, after controls were drained. It takes
   incremental steps until one returns `Complete` or `Unsupported`, until the
   batch has spent 1 ms, or until the runtime's `Wake` has a pending
   notification. Every send to the output, message and control queues sets
   that flag, and `std::sync::mpsc` has no emptiness check, so `Wake` gains a
   non-consuming `is_pending`. `Pending` sets the deadline to now plus 1 ms,
   so with ordinary pages a pass uses about half a core and yields between
   batches, as Ghostty's 1 ms step interval does. `Complete` clears the
   deadline. `Unsupported` disables the schedule for that terminal. A step
   error is a runtime failure, like any other engine error.

Output that arrives mid-pass waits for at most the step in progress. Ghostty
keeps its continuation as a page serial and restarts from the first page if
the list changed between steps, so no state needs care on Huterm's side. The
output's own token change pushes the next batch 250 ms out.

Every terminal compresses on its own runtime thread, including hidden tabs,
background windows and terminals whose child has exited, which are the best
candidates. Nothing crosses threads. After a font change or window resize
that touches many panes, their passes run at once on separate threads, each
at about half a core, for about 0.3 s with colored history at the maximum.

### Configuration

```toml
[terminal]
scrollback_compression = true # default
```

The schema entry carries `x-huterm-section = "scrollback"`, so the Settings
window shows it without UI code. `TerminalCommand` gains a
`scrollback_compression: bool` field applied before the child starts, and
reload sends a `BufferEdit::ScrollbackCompression(bool)` to every terminal
through the same path, queue-full retries and viewer replacement included,
as `BufferEdit::Scrollback` uses for limits. Turning it off clears the
deadline. Turning it on forgets the remembered token and arms the deadline,
as Ghostty's application does, so history restored while it was off is
reconsidered without waiting for new activity. A reload that repeats the
current value changes nothing. Pages already compressed stay compressed
after a disable until something reads them; restoring them all would spike
memory for no benefit.

`docs/configuration.md` documents the setting in its Scrollback section and
changes "a full 100,000 rows uses about 110 MB per terminal" to state the
compressed and uncompressed figures, that a width change temporarily
restores everything, and which memory figure drops on macOS.

### Diagnostics

The engine diagnostic in `crates/huterm-gpui/src/desktop.rs` reports
`compression=enabled`, `disabled` or `unsupported` instead of the fixed
`compression=disabled`, from the configuration and the re-exported
`COMPRESSION_SUPPORTED`.

Memory statistics reach benchmarks only. A user-visible memory readout, or a
budget across terminals that compresses the largest first, can build on
`memory_usage` later.

### Documentation

- `docs/agents/terminal-engines.md`: replace "disables scrollback
  compression" with the scheduling rules, what restores pages, and the resize
  cost.
- `docs/agents/core-runtime.md`: compression runs only on the runtime thread,
  only from the idle branch, in spaced, bounded batches, never on the output
  path.

## Testing strategy

Unit and contract tests, all of which `test:ghostty-releasesafe` also runs
against ReleaseSafe Ghostty:

- `huterm-ghostty` contract tests on a terminal with several pages of history:
  stepping reaches `Complete`; memory usage then reports compressed pages and
  less resident memory; reading a cell from a compressed page returns the
  original cell and restores exactly that page; the token changes after output
  that allocates a page and after a scroll, and not after reads or a memory
  query; `compression_supported` equals `COMPRESSION_SUPPORTED`.
- `huterm-core` engine tests, from the prototype's twin comparisons at a few
  thousand rows: snapshot rows, cursor and link lookups at several scroll
  positions, compared before anything extracts full text so that rendering
  restores the pages, then the full text, match an uncompressed twin after
  compression, after width changes in both directions, after an
  alternate-screen round trip with a pass run while the alternate screen is
  active, after Clear Scrollback,
  after reset, after lowering the scrollback limit, and after ED 22 and ED 2
  at a shell-integration prompt with an active cursor style and hyperlink.
  Fixtures include styled, grapheme and OSC 8 history.
- `CompressionSchedule` unit tests with explicit instants: a changed token
  restarts the 250 ms deadline; input postpones a pending deadline; input
  after `Complete` schedules nothing; an unchanged token after `Complete`
  schedules nothing; a restore-only operation restarts it;
  `Pending` schedules the next batch 1 ms later; `Unsupported` stops it;
  disabled schedules never set a deadline; enabling arms a deadline even with
  an unchanged token; on, off, then on again without output arms one; a
  reload with the current value leaves the schedule unchanged.
- A unit test of the batch loop with a scripted stepper and wake flag: it
  stops after the step during which the flag was set, and after the step
  that crosses its 1 ms budget. `Wake::is_pending` can still be set at the
  start of a batch by a message handled earlier in the same loop pass, since
  only a wait clears it, so the test covers a batch that starts with the flag
  set and runs exactly one step; that case is harmless.
- One runtime test with a real PTY, a test-only short idle interval and an
  unlimited batch budget, so only the wake check can end a batch. A
  test-only hook records the order of compression steps and handled messages
  and, like the existing `RuntimeControl::Pause`, can hold the runtime after
  the first step that returns `Pending`, inside a batch, until the test
  releases it. Releasing goes through a plain channel, so it does not set
  the wake flag itself. The shell prints several
  pages of history. While the runtime is held mid-batch, the test enqueues
  PTY output through the test-only output sender and a control, then
  releases it, and asserts that both are handled before the next step and
  that the output appears in the next snapshot. It then waits, under a
  bounded polling deadline, until a test-only memory control shows
  compressed pages. A perturbation that skips the wake check must fail at
  the ordering assertion.

Benchmarks:

- `bench:engine` runs every test whose name contains `engine_benchmark` in
  one process, and the `resize-history` peak budget assumes earlier fixtures
  hold at most 10,000 rows. The new benchmarks are therefore named
  `compression_benchmark_*`, which that filter does not match, and a new
  `bench:compression` task runs each in its own `--exact` process.
  `ci:benchmarks` runs it after `bench:engine`, and the budget comment in
  `benchmark.rs` says why the fixtures are separate.
- `compression_benchmark_resize`: the build-log fixture at the maximum,
  drained before each resize, gated on its median and on the `VmHWM` peak
  after writing `5` to `/proc/self/clear_refs` just before the first resize,
  so the gate measures the resize rather than the fill. Budgets come from CI
  runs on the Linux runners, as #270's did. macOS reports `ru_maxrss` and the
  sampled footprint without gating them, because Darwin reclaims lazily.
- `compression_benchmark_steps`: build-log, colored and styles-per-cell
  history at the maximum, reporting resident before and after, step p50, p95
  and maximum, and the first-touch cost of scrolling 1,000 rows back, to the
  middle and to the top for each, asserting that each restored at least one
  page so a fixture change cannot time nothing. It gates the build-log saving
  at 70% or more and the step p95 of build-log and styles-per-cell at CI-set
  budgets. The maximum is not gated because preemption nearly doubled it in
  the Linux Docker run, and styles-per-cell's 189 steps leave p99 only two
  samples of headroom. A pin bump that silently stops compressing or slows
  the codec on ordinary or grown pages fails CI.
- `bench:scroll` needs no compressed variant, as finding 2 explains. Its
  configuration, and every smoke configuration, pins
  `scrollback_compression = false`, so a pass that starts mid-benchmark
  cannot change existing results.
- `bench:output-latency` gains a variant that measures what a pass can
  delay. The existing workloads cannot: their first interval, the only one
  a pass could overlap, is dropped from the summary, typing postpones the
  pass, and the flood workload runs on the alternate screen, which builds
  no primary history and never changes the token. A short burst does
  not help either: after 2,000 new lines the pass took 0.24 ms on macOS and
  0.26 ms on Linux, inside one batch and far faster than the `keys` driver's
  90 to 112 ms timer.

  The variant saturates history once, then repeats a cycle about 50 times.
  Each cycle changes the grid width by one column through the path a window
  resize uses, which restores every page, so the next pass must recompress
  all history. For build-log history that pass is 47 ms of steps, about
  95 ms of wall time with the 1 ms gaps. The runtime counts steps and tags
  each `RuntimeMessage::Input` it handles while a pass is pending; a new
  test-only channel carries both to the driver, which polls the step count
  every 1 ms. When the count rises, the driver sends one keystroke and times
  its echo. With compression on, only tagged cycles count, and the run fails
  if fewer than 40 qualify. With compression off, the driver sends the key
  at the moment the on run sent it on average after the resize, and the
  step and tag requirements do not apply.

  Under the input rule, a tagged keystroke can wait for at most the rest of
  one step. The variant therefore runs on styles-per-cell history, whose
  4 ms steps on the fill pass are the risk, as well as on build-log history
  for the typical case. The styles-per-cell pass after a resize was not
  measured, so each cycle reports its step p50 alongside the echo time. The
  variant records per-cycle samples rather than reusing `summarizeKeys`'s
  one-second intervals, and the PR reports p50 and maximum with compression
  on and off. Nothing is gated until CI numbers exist.
- On macOS, `bench:idle` runs before and after to show that a completed pass
  adds no wakeups.

Platform coverage: macOS arm64 natively, Linux arm64 through
`mise run linux:test`, and Linux x86_64 in CI. All report support.

## Delivery

Both PRs build on the Ghostty 1.4.0 bump, #280.

1. **Bindings and measurement.** The `huterm-ghostty` API, regenerated
   bindings, contract tests, engine forwarding marked
   `#[expect(dead_code)]` until PR 2 calls it, the twin-comparison engine
   tests, and the compression benchmarks with CI-derived budgets. No
   user-visible change, so a `test:` or `refactor:` title keeps it out of
   release notes.
2. **Idle compression.** `CompressionSchedule`, the runtime hook, the setting,
   its schema, reload, the diagnostic, the pinned benchmark and smoke
   configurations, the output-latency variant, documentation and the runtime
   test. Title `feat: compress idle scrollback history`.

One PR is possible: the first is mostly mechanical and has no behavior. The
split lets the budgets settle on CI before anything depends on them.

## Open questions

Decided on 2026-10-27: each recommendation below was accepted as written.

1. **Default on or off?** Recommendation: on.
   - For: captured output shrinks by 92% to 95%, Ghostty's own application
     compresses by default, and the twin comparisons found no difference in
     text, styles, links or cursor across reflow, the alternate screen,
     clear, reset, `scrollClear` and limit changes. At the default 10,000
     rows the resize cost is a few milliseconds.
   - Against: the code is new upstream. Ghostty's fuzzers never call
     compression, and its unit tests never reach the real `madvise` path. A
     decode bug would trap inside Ghostty and end the whole process, not one
     terminal.
2. **Should there be a setting?** Recommendation: yes,
   `scrollback_compression`, because it costs one schema entry and gives users
   and support a switch if a regression appears.
   - For: Ghostty exposes the same switch; the Settings window picks it up
     from the schema; disabling it needs no restart.
   - Against: one more setting most people never touch. An environment
     variable would serve as a hidden escape hatch, but Huterm does not use
     environment variables for behavior settings.
3. **Idle interval, and what counts as activity?** Recommendation: 250 ms,
   started by token changes and restore-only operations, and postponed by
   client input.
   - For: memory returns soon after output stops, and counting input keeps
     passes away from typing. A pass that finds nothing new takes about
     15 µs.
   - Against: bursty output with pauses just over 250 ms compresses pages
     that the next burst may recycle, spending CPU twice. A 1 to 2 s
     interval would avoid that and make an interrupted keystroke rarer, at
     the cost of holding memory longer. Ghostty's application does not count
     input.
4. **Accept the resize cost at the maximum?** Recommendation: accept it, gate
   it, and track the upstream fix.
   - For: a resize is user-initiated and a drag pays once. At the default
     10,000 rows it adds 2 to 5 ms. Users who raise the limit to 100,000 are
     the ones who save 90 MiB or more per terminal.
   - Against: at the maximum a resize after a pass takes about 32 ms on
     Apple silicon for ordinary logs and 108 ms for colored history, and
     roughly 60 to 140 ms for ordinary logs on slower x86_64 hosts.
     [Scrollback history scale](scrollback-history-scale.md)'s half-frame
     target, set on plain history, is already missed by uncompressed
     build-log (10.0 ms) and colored (57.2 ms) history, but compression
     triples the build-log reflow time, to about four times the target.
     Alternatives are to lower
     `ScrollbackLimit::MAX` while compression is on, to skip compression
     above some history size, or to wait for upstream deferred reflow of
     off-screen pages (the TODO in
     `PageList.resize`), which is Jim's to propose. Compression gives no
     reason to raise the maximum.
5. **What about very large pages?** Recommendation: accept unbounded steps
   for now, gate the styles-per-cell step p95, and revisit only if a real
   workload shows long steps.
   - For: the largest measured step, 4.1 ms on per-cell truecolor pages,
     delays at most one message, and only after an idle period. Skipping
     large pages would need a native patch or an upstream option.
   - Against: page size has no limit, so a pathological stream could produce
     a step of tens of milliseconds. A Huterm patch that skips pages above a
     size would bound it, at the cost of a patch to carry across bumps.
6. **Show memory to users?** Recommendation: not now.
   - For deferring: no current consumer, and Activity Monitor already shows
     the physical footprint. The API is there for a later readout or a
     cross-terminal budget.
   - Against: on macOS, `ps` and other RSS readers keep showing the
     uncompressed figure until the system reclaims memory, which may confuse
     users comparing numbers.
7. **Two PRs or one?** Recommendation: two, as in [Delivery](#delivery).
   - For: budgets settle on CI before behavior depends on them, and each PR
     reviews quickly.
   - Against: the first PR adds API with no production caller for a short
     time.

## Implementation outcome

Both PRs follow the design above, with these differences:

- **Benchmark counters.** The output-latency variant needs to know when a pass
  is running without disturbing it, and polling the runtime with a control
  sets the wake flag and ends every batch. `TerminalViewer::compression_stats`
  instead reads steps, step time, and input handled mid-pass from atomics on
  the runtime's registry.
- **Activity forwarding.** PR 1 forwarded only `compress_step` and
  `memory_usage`; nothing in core read the token until PR 2's scheduler. In
  core, `memory_usage` stays test-only.
- **Reload coverage.** GPUI tests check that a reload changing only
  `scrollback_compression` reaches the viewer and survives a viewer
  replacement. They cannot see a pass, because compression never shows in a
  snapshot; a core runtime test proves the same viewer call starts one.
- **Settings.** The setting sits in the Scrollback section's own "Memory"
  group.
- **Smokes.** Smoke configurations keep the default instead of pinning
  compression off, as [Testing strategy](#testing-strategy) said, so they
  exercise what users run. Their histories are a few pages, so a pass costs
  them microseconds; the benchmarks whose numbers a pass could shift still pin
  it off.
- **Latency variant.** The driver waits for history to stop growing, because
  per-cell truecolor history reaches the size allowance at about 67,000 rows,
  and the run ends when the driver finishes its 50 cycles. A key counts when
  the runtime handled it while a pass was unfinished, which includes the idle
  interval after earlier input, not only a running step.

`bench:output-latency -- compression` on an Apple M3 Max, 50 cycles each;
three cycles per run saw no echo within 5 s, with or without compression:

| History | Compression | Keys mid-pass | Applied p50 | Applied max | Mean step |
| --- | --- | --- | --- | --- | --- |
| Build log | on | 47 | 180 µs | 3.1 ms | 125 µs |
| Build log | off | | 181 µs | 1.7 ms | |
| Per-cell truecolor | on | 47 | 241 µs | 5.8 ms | 4.4 ms |
| Per-cell truecolor | off | | 298 µs | 5.4 ms | |

Keys mostly arrive in the 1 ms gap between batches, so even 4 ms steps leave
the median unchanged; the worst case grows by up to one step.
