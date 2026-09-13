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

The private entry inherits `xterm-256color`, Huterm's previous advertised
baseline, and adds only `RGB` and `Tc`. Its `pairs` value is capped at 32767 so
native `tic -x` on macOS and Linux emits the portable 16-bit compiled format.
Package builds compile it on their native host. macOS stores it under
`Contents/Resources/terminfo`; Linux tarballs and AppImages store it under
`share/huterm/terminfo` relative to the executable.
Both locations include the reviewed source and the ncurses redistribution
notice alongside the compiled entry.

When adding the packaged directory, preserve `TERMINFO` unchanged and append to
an inherited `TERMINFO_DIRS`. An empty component continues to mean the ncurses
default directories. Development discovery uses the compile-time repository
location, so it does not depend on the launch working directory.

Run `mise run terminfo:check` to compile and inspect the entry and to capture a
literal truecolor SGR sequence through an isolated tmux server and real outer
PTY. The check uses a private socket, empty tmux configuration, and temporary
state; it never connects to the user's tmux server.

## Native inputs and policy

The safe Rust bindings and locally patched sys crate are pinned to 0.2.1.
Native Ghostty is pinned to
`22d13172cde98a0a4dda05d3d6a3fcb0dd8ed018` and built with Zig 0.16.0 using
static linking. Both Rust crates incorporate upstream revision
`5988a0b78b4aa804d1c12e66bbfe662bd97d81c0` over their published archives.
`scripts/ghostty-source.json` records the reviewed archive and source-tree
hashes. Required notices and provenance live in `third-party/ghostty`.

Normal Mise build, test, smoke, benchmark, and package tasks prepare these
inputs. Before a direct Cargo build, run:

```sh
mise run ghostty:prepare
mise run build:exec -- cargo build --locked
```

Preparation verifies the ignored `.native/ghostty` tree rather than accepting
local drift. The sys build copies it into a private Cargo build directory so
Zig-generated state cannot modify the verified source. Build isolation also
prevents enclosing Git tags from changing Ghostty's generated version data.
The adapter disables Kitty graphics and scrollback compression, uses the
portable CPU baseline, and retains a 16 MiB scrollback byte budget.

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

Core seeds foreground, background, cursor, palette, grid, and physical cell size
before spawning the child. It answers native OSC 4,
10/11/12, CSI 14/16/18t, and CSI ?996n queries from that ordered retained state.
Same-chunk mutations affect later queries in the same input chunk. Explicit OSC
overrides remain distinct from theme defaults even when their RGB values are
equal; resets reveal the newest published default without changing terminal
text. Appearance queries classify the effective terminal background by
luminance, including an active OSC 11 override; they do not report the operating
system appearance.

One attachment-scoped `PresentationController` publishes coherent replacements
for a terminal's defaults. A replacement controller revokes its predecessor,
and detach, retarget, move, terminal close, and controller drop revoke queued
updates again when the runtime applies them. This authority is independent of
clipboard permission. The desktop publishes successful reloads to hidden tabs
and retries bounded queue pressure; font and display-scale changes use the same
ordered resize path as visible terminals. Detaching or revoking a controller
retains the last accepted presentation; it prevents stale future updates rather
than restoring an older theme.

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

OSC 52 and OSC 1337 clipboard parsing remains inside the adapter, but only the
desktop host can authorize and deliver clipboard writes. Reloading clipboard
policy applies to existing terminals. Hidden and background views cannot bypass
that authority.

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
