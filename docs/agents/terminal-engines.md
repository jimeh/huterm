# Ghostty terminal engine

Every Huterm terminal uses the private Ghostty adapter. The runtime owns PTY
spawning, input encoding, lifecycle, the shared viewport, and immutable Huterm
snapshots; native Ghostty handles remain on the terminal owner thread inside
`huterm-core`.

## Configuration compatibility

New configuration should omit `terminal.engine`. The two former values remain
accepted temporarily so existing files continue to load:

```toml
[terminal]
engine = "alacritty" # Deprecated. This still launches Ghostty and warns once.
```

An explicit `"ghostty"` value also launches Ghostty without warning. Omission
is the normal path and produces no warning. Unknown values, non-string values,
and malformed TOML remain configuration errors. Startup validates the legacy
field even when an unrelated setting falls back to defaults, and preserves an
explicit clipboard deny on that fallback path. Reload is transactional: an
invalid file retains the active configuration, while a later successful reload
without `engine` clears the migration warning. Huterm never rewrites the file.

## Terminal identity and terminfo

Terminal identity remains a client launch concern. The desktop resolves
`terminal.term` before constructing `TerminalCommand`, and core applies the
resolved environment before spawning the child. `auto` selects
`TERM=xterm-huterm` only when the entry exists in the inherited ncurses search
paths or Huterm's private resources; otherwise it uses `TERM=xterm-256color`.
The explicit values force either identity for new terminals.

The private entry inherits the indexed `xterm-256color` capabilities that Huterm
previously advertised, and adds only `Tc` for truecolor-aware consumers. It
deliberately omits ncurses's
[`RGB` capability](https://invisible-island.net/ncurses/man/user_caps.5.html#h2-Recognized-Capabilities),
which would assert that the inherited `setaf` and `setab` strings take direct
RGB values rather than indexed colors. Its `pairs` value is capped at 32767 so
native `tic -x` on macOS and Linux emits the portable 16-bit compiled format.
Package builds compile it on their native host. macOS stores it under
`Contents/Resources/terminfo`; Linux tarballs and AppImages store it under
`share/huterm/terminfo` relative to the executable.
Both locations include the reviewed source and the ncurses redistribution
notice alongside the compiled entry.

When adding the packaged directory, preserve `TERMINFO` unchanged and append to
an inherited `TERMINFO_DIRS`. An empty component continues to mean the ncurses
default directories. Development discovery finds `target/terminfo` relative to
debug, release, and example executables, so it does not depend on the launch
working directory.

Run `mise run terminfo:check` to compile and inspect the entry and to capture a
literal truecolor SGR sequence through an isolated tmux server and real outer
PTY. The check uses a private socket, empty tmux configuration, and temporary
state; it never connects to the user's tmux server.

## Native inputs and policy

Huterm binds the C API through its own `huterm-ghostty` crate. Native Ghostty
is pinned to `cec8b92f1ecd176a20a56aa405021e7ae991b996` and built with Zig
0.17.0 using static linking. The crate's FFI declarations are generated from
the pinned headers with `mise run ghostty:bindings` and checked with
`mise run ghostty:bindings:check`. `scripts/ghostty-source.json` records the
reviewed archive and source-tree hashes. Required notices and provenance live
in `third-party/ghostty`.

Normal Mise build, test, smoke, benchmark, and package tasks prepare these
inputs. Before a direct Cargo build, run:

```sh
mise run ghostty:prepare
mise run build:exec -- cargo build --locked
```

Preparation verifies the ignored `.native/ghostty` tree rather than accepting
local drift. The `huterm-ghostty` build script copies it into a private Cargo
build directory and applies Huterm's [native patches](#native-patches) there,
so neither they nor Zig-generated state modify the verified source. Build
isolation also prevents enclosing Git tags from changing Ghostty's generated
version data. In a repository with several checkouts, preparation reuses a
matching archive from another one instead of downloading; see
[linked worktrees](development.md#linked-worktrees).
The adapter disables Kitty graphics, the Glyph protocol, APC buffering, and
scrollback compression, uses the portable CPU baseline, and limits history by
the configured rows and optional size, as
[the huterm-ghostty crate](#the-huterm-ghostty-crate) describes.

## Runtime behavior

Ghostty publishes complete owned snapshots with shared immutable rows. Huterm
retains its existing behavior for dynamic palette overrides, fragmented OSC
sequences, Unicode and soft wraps, selection generation checks, viewport
anchoring, alternate screens, and bounded OSC 8 lookup. Snapshot failures close
the runtime through normal cleanup. Engine initialization completes before the
PTY child starts, and startup publishes only after the I/O workers are ready.

Ghostty can leave render rows clean after a color-only OSC update, and its API
does not expose which defaults were explicitly overridden. The adapter retains
OSC and reset invalidation hints across fragmented input, probes changed
defaults, restores the defaults before rendering, and compares effective colors
before consuming damage. This keeps explicit overrides and cached row colors
coherent without forcing unrelated rows to rebuild.

Ghostty resolves a `Point::Screen` grid reference by walking page nodes from
the top of history, but a `Point::Viewport` reference from the viewport's own
position. Link lookup therefore reads rows at or below the current viewport
top, scrolled or not, through viewport points and keeps screen points for rows
above it. Plain-text lookup reads only the token under the pointer, up to the
nearest delimiters, and applies the scan limits to that token; the bounding
delimiters do not count. Reads above the viewport still cost one walk each as
history grows, because there is no native API to step a reference between rows.
They occur for every cell of a token that extends above the viewport, and once
when a token reaches the viewport's top-left cell and the row above must be
checked for a soft wrap. That check reads the row above's own wrap flag: the
next row's continuation flag can disagree with it after line insertion or
deletion. The walks are why `ScrollbackLimit::MAX` is 50,000 rows: the
`scan-limit-above-viewport` fixture's p95 was 1.9 ms locally with 50,000 rows
of history and 4.7 ms with 100,000, against the benchmark's 5 ms gate.

Core seeds foreground, background, cursor, palette, grid, and physical cell size
before spawning the child. It answers native OSC 4,
10/11/12, CSI 14/16/18t, and CSI ?996n queries from that ordered retained state.
Same-chunk mutations affect later queries in the same input chunk. Explicit OSC
overrides remain distinct from theme defaults even when their RGB values are
equal; resets reveal the newest published default without changing terminal
text. Appearance queries classify the effective terminal background by
luminance, including an active OSC 11 override; they do not report the operating
system appearance.

Each viewer submits the presentation it would apply, and the runtime applies the
controlling viewer's: the viewer that most recently gained focus or typed. A
viewer that does not control keeps submitting without failing, and its
presentation applies once it takes control. Revoking or dropping a viewer
retains the last applied presentation; it prevents stale future updates rather
than restoring an older theme. This authority is independent of clipboard
permission. The desktop publishes successful reloads to hidden tabs and retries
bounded queue pressure; font and display-scale changes use the same ordered
geometry path as visible terminals.

The current protocol always carries a grid and `CellSize`, but zero cell width
or height means that physical geometry is unavailable. The runtime then leaves
CSI 14/16/18 unanswered because Ghostty uses one callback for all three queries;
it never combines a known grid with unavailable pixel geometry. Desktop windows
publish nonzero cell dimensions before child spawn. Size replies report only
the canonical grid geometry, never outer-window or chrome dimensions.

Ghostty's public mode bits do not fully describe active mouse tracking and
format. The adapter uses a retained native probe with synthetic geometry, then
Huterm encodes real input at dequeue time. Probe bytes never reach the PTY.
Disabling an inactive 1005 or 1006 format resets to legacy encoding. Primary
device attributes report VT220 with ANSI color; Kitty keyboard and graphics
capabilities remain unadvertised. SGR-pixel mouse mode 1016 remains unsupported;
implementing it requires protocol and coordinate decisions outside this engine
removal.

OSC 52, OSC 1337, and Kitty OSC 5522 clipboard parsing remains inside the
adapter, but only the desktop host can authorize and deliver clipboard writes.
Kitty writes follow the same policy and answer the program with a status.
Reloading clipboard policy applies to existing terminals. Hidden and background
views cannot bypass that authority.

## Measurement and smoke coverage

Run the single-engine performance tasks directly:

```sh
mise run bench:engine
mise run bench:scroll
mise run bench:renderer
mise run bench:links
```

The engine benchmark reports the fixed Ghostty revision, p50/p95 elapsed
processing and snapshot times, row reuse, throughput, and retained history.
The scroll and link gates keep their existing thresholds and queue limits.
The link benchmark runs its viewport, above-viewport, long-line, and scrolled
fixtures with empty scrollback and with scrollback saturated at the largest
limit, so their elapsed times can be compared. Raising that limit lengthens the
above-viewport fixtures' walks.
Elapsed time includes scheduler preemption; Xvfb snapshot evidence does not
prove continuous presentation.

Native input, clipboard, integration, fullscreen, quake, palette, and Quit
smokes all use the omitted-engine configuration path. The macOS input smoke also
runs one short launch with legacy `engine = "alacritty"` and requires the
runtime diagnostic to identify Ghostty and the reviewed revision. Use
`mise run smoke:manual-integration` for a recorder that never executes dropped
paths.

`mise run smoke:presentation-queries` drives a production native window and PTY.
It compares direct OSC 10/11 plus CSI 14/16/18 replies with the rendered grid
and physical cell metrics, then repeats them after an inactive tab receives a
theme and font reload without activation. Linux runs this under Xvfb; macOS runs
it against AppKit. This proves local PTY behavior only. It does not claim a
Codex UI session, remote SSH host, or Mosh behavior.

Historical plans and measurements may still name Alacritty. They describe the
state at the time and do not define the shipped runtime after issue #118.

## Pin and native build

Ghostty builds use native revision
`cec8b92f1ecd176a20a56aa405021e7ae991b996` and Zig 0.17.0. Keep its
`memset` C ABI fix: the first Zig 0.16 migration pin mishandled negative fill
values and corrupted Rust hash-table control bytes. Run
`mise run ghostty:prepare` before direct Cargo build commands; it checks the
full native source tree against `scripts/ghostty-source.json`. The build script
rejects source that prepare did not verify against the current manifest, such
as a stale tree that rust-analyzer builds right after a pull. Keep that
source, `huterm-ghostty`'s generated bindings, and bundled notices aligned. All
builds require the pinned Ghostty source and Zig toolchain. Native source
dependencies use Zig's content hashes; their notices are in `third-party/ghostty`
because they are outside Cargo's license audit.

Keep verified native source inputs in `.native/ghostty`, outside Cargo's
`target` directory. The pinned rust-cache action recursively removes non-Cargo
files under `target` before saving, leaving incomplete native source trees on
restore. Preserve source hash checks; never repair mismatches silently.

Keep Cargo's Git discovery ceiling at `.native/ghostty`. The extracted source
has no repository, and Ghostty otherwise discovers Huterm's enclosing release
tag and panics because it does not match Ghostty's version. The ceiling preserves
Git discovery from Huterm's root and uses Ghostty's archive-version fallback.

Zig 0.16 and later create mutable `zig-pkg` dependencies beside `build.zig`.
Build from a fresh private source copy under `huterm-ghostty`'s `OUT_DIR`, with
the Zig child's Git discovery ceiling set there. Keep the verified
`.native/ghostty/source` unchanged and compilation caches outside the refreshed
copy.

Build commands retain `scripts/build-exec.sh` as their shared entrypoint and
preserve the selected Xcode, including explicit `DEVELOPER_DIR` overrides.
The pinned Ghostty source and Zig 0.17 support Xcode 27 without SDK fallback or
an `xcrun` shim. Check that `xcrun metal --version` actually runs: the launcher
can exist even when Xcode's optional Metal Toolchain is missing.

Cargo forces `HUTERM_GHOSTTY_CPU=baseline` for portable native instructions and
`HUTERM_GHOSTTY_OPTIMIZE=ReleaseFast`. Local builds retain warm native artifacts.
Cargo must force the verified native source path and benchmark optimization
over inherited environment values.

The pinned CI cache action prunes path dependencies inside the repository, so
`huterm-ghostty`'s build script reruns in every CI job. With
`HUTERM_GHOSTTY_ARTIFACT_CACHE` set, it links a cached `libghostty-vt.a` whose
fingerprint matches the build and otherwise builds from source and stores the
result. The fingerprint covers the source manifest, Zig version and arguments,
target, and the host libc or macOS SDK: Linux host builds stay native, so an
archive built against a newer glibc must never reach a 2.35-ceiling package.

Before `zig build`, the build script runs `zig build --fetch` with the same
arguments and retries it after 5 and 20 seconds. Ghostty's Zig packages come
from upstream hosts: most from deps.files.ghostty.org, and translate-c's `aro`
dependency from codeberg.org over Git. One 503 from codeberg.org once failed
every compiling CI job in a run. Fetch with the build's exact arguments: a
separate prefetch with default options missed lazy packages, such as
`pixels`, that the real build needs. Zig keeps fetched packages as tarballs in
its global cache's `p` directory, which is enough for a later offline build;
`restore-ghostty` and `save-ghostty` persist that directory in CI, keyed by OS,
architecture, and `scripts/ghostty-source.json`.

CI jobs restore and save `.native/ghostty-prebuilt` through the
`restore-ghostty` and `save-ghostty` actions, keyed by namespace and by a hash
of the stored slots, so a changed input or rebuilt slot saves a new entry
instead of leaving a stale one. A restore takes the namespace's newest entry,
so jobs share a namespace only when they build the same slots on the same
runner image. Release workflows leave the variable unset and always build from
the verified source.

## Native patches

Reviewed fixes to Ghostty's native source live as Git-style patches, with
`a/` and `b/` prefixes against the source root, in
`third-party/ghostty-patches`.
They stay out of `third-party/ghostty` because `Cargo.toml` bundles that
directory into `Huterm.app`. `scripts/ghostty-source.json` lists them in
application order with a name, file, SHA-256, description, and an upstream link
once one exists. [Their README](../../third-party/ghostty-patches/README.md)
records why each patch exists and when to drop it.

`ghostty:prepare` checks each patch's SHA-256 and the paths it touches, then
publishes verified copies and an ordered `series` file to
`.native/ghostty/patches` before it writes the stamp. It writes the series even
when the list is empty, leaves unchanged files and their mtimes alone, and
removes the stamp before it replaces a changed set. `ghostty:check` compares the
published copies without writing. Prepare rejects paths that leave the source
root, and paths under `include/`: bindgen reads the pristine headers in
`GHOSTTY_SOURCE_DIR`, so a header patch would not reach the generated bindings.

The build script applies the series to its private `OUT_DIR` copy with
`git apply`, so a source build needs Git whenever the list is non-empty;
`mise run doctor` checks for it. A missing series or a failing patch stops the
build with the patch's path and Git's output. `git apply` exits successfully
and changes nothing when it discovers an enclosing repository and runs from a
subdirectory, which is where Cargo's `target` sits in Huterm's checkout. The
build therefore removes inherited `GIT_*` variables, sets the discovery ceiling
at the copy's parent, ignores system and global Git configuration, and requires
every path a patch touches to differ afterward from its bytes before that
patch, so a later patch may restore pristine bytes.

The series is not a Cargo rerun trigger. The manifest and stamp already change
with any patch edit, and a trigger path that can be missing reruns Zig on every
build. Because the manifest records patch hashes, a patch-only edit also
changes the cached archive fingerprint and every CI cache key hashed from the
manifest: CI rebuilds Ghostty and refetches its Zig packages once.

To add a patch, extract the pinned archive into a scratch directory, commit it
to a new Git repository there, make the fix, and save `git diff` output as
`third-party/ghostty-patches/<name>.patch`. Add its manifest entry with the
file's SHA-256 and its README notes, then run `mise run ghostty:prepare` and
Huterm tests that fail without it. Huterm builds only lib-vt and never runs
Ghostty's Zig tests, so run the patch's own tests in the scratch tree with
`zig build test-lib-vt -Dtest-filter=<test name>`.

On a pin bump, rebase each patch in order the same way: commit the new source
in a scratch repository, `git apply --3way` the patch, resolve conflicts,
regenerate it with `git diff`, update its SHA-256, and rerun its Zig tests
there. Drop a patch, with its manifest entry and README notes, once the pin
includes an equivalent upstream fix.

## The huterm-ghostty crate

`huterm-ghostty` owns everything that changes with the pin: native build, FFI,
safe wrapper, and API-gap probes. Its `unsafe` code stays in `native.rs`,
`callbacks.rs`, generated `ffi/bindings.rs`, and test-only `test_alloc.rs`;
each opts out with `#![expect(unsafe_code)]`, denies undocumented and
multi-operation unsafe blocks, and cites the header contract in every
`// SAFETY:` comment. Callbacks reach host code only as plain values, contain
panics by poisoning the terminal, drop panic payloads inside a second
`catch_unwind`, and queue effects until the write returns. Kitty OSC 5522
writes reach the same host path as OSC 52: `disable_apc_protocols` turns off
only Kitty graphics, the Glyph protocol, and APC buffering. The engine sets
the Kitty write limit from `host_effects::TERMINAL_BYTE_LIMIT`, so larger
writes get `EFBIG` from Ghostty instead of a misleading `EBUSY`. That budget
is 64 MiB, the minimum `terminal.h` says the protocol requires, and the
process budget holds two such writes; never lower it below Ghostty's default.

It admits the first `text/plain` representation whose charset, if any, is
UTF-8 (quote-aware parameter parsing; a valueless `charset` is refused), under
`terminal.clipboard_write`, and ignores Kitty names, passwords, and grants;
replies never set `remember`. Clipboard reads and Kitty paste events stay
uninstalled. Keep the mouse
encoder and events private to `MouseProbe`'s fixed geometry: Ghostty converts
encoder geometry and positions with unchecked float-to-integer casts.

`mise run ghostty:bindings` regenerates `src/ffi/{bindings,keys,layout}.rs` with
bindgen, which loads libclang at run time; on macOS the task pins
`LIBCLANG_PATH` to the selected Xcode, because clang-sys otherwise prefers any
`llvm-config` on `PATH`. `ghostty:bindings:check` compares bytes. Getter and
option value types come only from the generated `keys.rs`.

Each key set declares where its header puts annotations (a final labeled line
or the first sentence's parenthesized type), and generation fails otherwise.
Outputs carrying a pointer the library writes through get a `*Populate` trait
that the generic getters reject, plus a dedicated wrapper. `key_tests.rs`
checks every key's type against the bytes the library writes or reads. Tests
read native-written memory only at declared fields and the tag-selected union
member, taken from the manifest: foreign writes may leave padding
uninitialized.

The build script reads `GHOSTTY_SOURCE_DIR`, `HUTERM_GHOSTTY_OPTIMIZE` (Debug,
ReleaseSafe, ReleaseFast, or ReleaseSmall), `HUTERM_GHOSTTY_CPU`,
`MACOSX_DEPLOYMENT_TARGET`, `ZIG`, and `HUTERM_GHOSTTY_ARTIFACT_CACHE`, and
reruns only when those, itself, `scripts/ghostty-source.json`, or the stamp
beside the source directory change. `ghostty:prepare` writes that stamp,
`.native/ghostty/source.manifest.json`, with the manifest's bytes once the tree
verifies. The build fails, even before reusing a cached archive, unless the
stamp matches the manifest. A source build also reads the patch series beside
the stamp, without a rerun trigger; see [native patches](#native-patches).
`scripts/ghostty-build.test.ts` runs its unit tests. The crate's contract tests
pin each C behavior the engine relies on, and the ABI test checks every emitted
FFI type against `ghostty_type_json()`. After a pin bump, fix a failing contract
test's assumption before changing engine code. Allocator vtable callbacks
receive log2 alignments, not the byte counts `allocator.h` describes. Set
history only through `apply_scrollback` in the engine, and set both Ghostty
limits every time: the C default is a 10,000-byte budget. Ghostty applies both
together, and whichever is reached first prunes. A nonzero row limit sets the
line limit, plus either the configured `scrollback_bytes` or, without one, a
derived byte cap of 4 KiB per history and screen row and 4 MiB of headroom,
which bounds pages that grow for styles, graphemes, and hyperlinks. The byte
limit also counts the screen's pages and whole partly filled pages: without the
headroom a 350-row limit on a 400 by 60 screen held only 297 rows, and without
the screen rows it held 166 on a 400 by 1500 screen. `resize` raises a derived
cap for the taller screen before resizing and settles it afterward, so a growing
screen does not prune history the row limit keeps. It sets explicit bytes again
after resizing too: Ghostty enforces bytes only when they are set or a page is
allocated, so a widening reflow, whose rows cost more, otherwise left 713 rows
where 2 MiB holds 591. Zero rows or zero bytes set the byte limit to zero,
because Ghostty never limits lines below one page of rows at the current width.
Line pruning removes only whole historical pages, so history stays at or below
the limit and less than a page under it; contract tests pin those bounds,
eviction and viewport fixup when the limit drops, the primary-screen target
while the alternate screen shows, reflow enforcement, and that RIS and ED 3 keep
the limits, and that the first-reached limit applies. Only fewer rows or a
smaller byte allowance at the current height can evict, so only that counts as a
content change, even when nothing visible moved, because the alternate screen
hides the primary history it prunes. A raised limit leaves the generation alone.
Ghostty's render state rebuilds every row when the viewport pin moves, so
eviction needs no forced snapshot rebuild. Set Ghostty device attributes
explicitly: a declining callback still gets Ghostty's default VT220 replies at
this pin, as a `huterm-ghostty` contract test shows.

## Color and mouse state

Ghostty color-only OSC updates can leave render rows clean. Compare effective
colors and retain explicit palette override information before consuming damage.
Its API lacks the override mask; the adapter probes changed defaults after
color OSC (4, 5, 10-19, 21, 104, 105, 110-119), RIS, or DECSTR hints, then
restores defaults before rendering. DECSTR (`CSI ! p`) resets the palette but
keeps dynamic colors. The probe's palette writes force a full redraw, so other
OSCs must not trigger it. The hint mirrors the pinned parser's transitions:
Ghostty decodes ground bytes as UTF-8, so raw C1 bytes there are text, OSC and
DCS payload bytes never start new sequences, and CAN or SUB cancel an OSC
without dispatching it. It may over-flag but must never miss a color operation;
the full-probe differential test enforces that, so add any new color-changing
sequence to its alphabet when a pin bump introduces one. Keep this hint state
across input chunks, including snapshots between fragments.

Ghostty's public mode bits can disagree with its active mouse format/tracking.
Read the active behavior through the retained native mouse probe, with synthetic
200x200 geometry independent of the real grid, then feed Huterm's shared encoder.
Never send probe output to the PTY. At the selected native pin, disabling an
inactive format resets to legacy encoding.
