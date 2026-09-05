# Terminal engines

Every Huterm build includes Alacritty 0.26.0 and libghostty-vt 0.2.1.
Alacritty is selected by default. Both engines operate without changing the
GPUI renderer or PTY ownership. Each terminal has one runtime-owned viewport
and publishes complete immutable snapshots. Unchanged rows share storage, so
clients may skip intermediate snapshots safely.

## Build and select an engine

```sh
mise run dev
```

Set the engine in the configuration file:

```toml
[terminal]
engine = "ghostty"
```

Reload configuration and open a tab or window. Configuration is captured before
a spawn starts; reload does not change existing or pending terminals. Use
`"alacritty"` to switch the default back. An explicit unknown
engine fails configuration validation. Malformed TOML also stops startup because
the engine choice cannot be recovered safely. With valid TOML and a valid
engine, unrelated settings errors print a diagnostic and use default settings
while preserving that engine. Reload errors retain the previous configuration.

Normal build, development, test, packaging, and benchmark tasks prepare the
reviewed native inputs and build both engines. No Cargo feature flag is needed.
For direct Cargo commands, prepare the inputs first and use the SDK wrapper:

```sh
mise run ghostty:prepare
mise run build:exec -- cargo build --locked
```

`mise run package:macos` builds and verifies an Apple Silicon app with both
engines and the native license notices. Benchmark engine selectors change only
configuration; both engines run from the same compiled binary.

Build tasks use `build:exec`'s shared Bash SDK wrapper.
Mise pins Bun and Zig; the wrapper only selects the SDK and executes its
command. On macOS, if the
selected SDK is 27 or newer, it selects Xcode 26 at
`/Applications/Xcode.app/Contents/Developer`. Zig 0.15.2 fails to link its build
runner against the Xcode 27 beta SDK, reporting undefined system symbols before
Ghostty compiles. If Xcode 26 is elsewhere, set `DEVELOPER_DIR` to its developer
directory. An explicit `DEVELOPER_DIR` is always preserved. This selection applies
only to the invoked command; it does not change `xcode-select` or other projects.
Direct Cargo builds must supply the compatible environment themselves or run
through `mise run build:exec -- <command>`.

The pinned binding builds Ghostty for the host CPU on Linux. After moving Cargo
artifacts between machines, run `mise run ghostty:clean` before rebuilding.
Linux CI does this after cache restoration to avoid illegal instructions from
another runner's native code. macOS uses Ghostty's generic Apple Silicon target.
Portable Linux binary distribution will need an explicit baseline CPU target.

## Native inputs and policy

The published Rust bindings and sys crate are pinned to 0.2.1. Native Ghostty is
pinned to `a887df42c56f6de86c0fe6da9c4eeca37931e083`, built with Zig 0.15.2 and
static linking. Source preparation runs on Bun and retains the OS-owned
preparation lock and reviewed source-tree hash format.
`scripts/ghostty-source.json` records the archive checksum and
full source-tree checksum. Preparation checks the Rust revision constant and
license provenance against that manifest, then verifies existing source contents;
a changed generated tree fails rather than silently building different source.
Generated inputs live in ignored `.native/ghostty`, outside Cargo's `target`
directory: the CI Cargo cache removes non-Cargo files under `target` before
saving. Previous `target/ghostty` contents are left untouched and no longer used.

This is a narrow native-source exception to the Cargo registry-only policy.
The bindings remain registry dependencies. The native archive has an immutable
revision and SHA-256; Zig dependencies use the content hashes in that reviewed
source. Zig may download application-related lazy packages during build
configuration, but the VT library does not link Ghostty's renderer or font stack.
Cargo's audit does not cover these native sources. Their linked dependency
notices and provenance are in `third-party/ghostty` and accompany the app bundle.

Cargo forces the reviewed source path and `ReleaseFast` optimization over
inherited environment values. Native code uses SIMD. Linux uses the native CPU;
native macOS builds use Ghostty's upstream baseline CPU workaround.
The adapter does not request scrollback compression. Compare engines on the same
host and record CPU information. Cross-host elapsed times are not equivalent
measurements.

## Behavior and compatibility

Both engines use Huterm's input encoder, modes, owned effects, snapshot cell
styles, selection extraction, and lifecycle cleanup. Native handles stay on the
runtime thread. Failed engine initialization occurs before PTY creation; startup
waits for I/O workers before returning a usable client. Resize effects use the
same ordered PTY writer as parser replies. Snapshot errors close the runtime
through the normal cleanup path and stop client snapshot resubmission.

The Ghostty history option is a byte budget at this pin, despite its binding and
header documentation calling it lines. The adapter uses 16 MiB. Alacritty retains
at most 10,000 history rows. Every benchmark reports actual retained history.

Ghostty's render API lacks an explicit palette override mask. After an OSC or
terminal-reset hint, the adapter briefly changes default palette values, reads
which effective entries remain fixed, and restores defaults before snapshot
extraction. Ghostty interprets the color sequences; the hint tracks only their
boundaries, including fragmented input. This preserves explicit application
colors even when they equal the engine's default palette. Effective color changes
also invalidate rows independently of native row damage.

Image rendering and Kitty keyboard input are not yet supported by Huterm. The
adapter disables the glyph protocol and APC payload storage and suppresses
extended device-attribute advertisements. Primary device attributes explicitly
report VT220 with ANSI color. It continues answering ordinary cursor
position queries and suppresses Kitty keyboard/graphics capability replies.
This does not add support for every terminal extension that
Ghostty understands internally.

Ghostty mode bits do not expose the active last-selected mouse format. A retained
native encoder probes local synthetic events to read the active tracking and
format, using fixed geometry even when the real grid is 1x1. These bytes never
reach the PTY; Huterm encodes real input. One intentional engine difference is
that Ghostty resets to legacy format when disabling an inactive mouse encoding,
while Alacritty preserves the active encoding. SGR-pixel mouse mode 1016 is not
supported: the shared input protocol encodes cell coordinates for legacy, UTF-8,
and SGR mouse reports. Applications requiring pixel coordinates cannot use that
mode correctly in Huterm.

Terminal scrolling is shared. Selection gestures, window navigation, active tabs,
and scrollbar animation remain client state. Multiple-attachment UI, terminal
size arbitration, live engine migration, and a serialized delta protocol remain
deferred.

## Measure the engines

Run timing trials separately, alternating engine order:

```sh
mise run bench:engine
mise run bench:engine:ghostty
mise run bench:scroll
mise run bench:scroll:ghostty
mise run bench:renderer
mise run bench:renderer:ghostty
```

The headless test runs 200 samples at 120x40 for ASCII output, styled text,
Unicode, sparse changes, and full-screen rewrites. Sparse and full-screen
fixtures alternate contents. Output checks run outside the measured processing
and snapshot intervals. Reports include p50/p95 elapsed nanoseconds, rebuilt and
reused rows, and actual history. These are elapsed times, including scheduler
preemption, rather than per-thread CPU time.

Desktop benchmarks create controlled engine configuration and report the actual
engine and immutable revision. The existing scroll queue and offset budgets are
unchanged. Relative scroll expectations use the runtime viewport immediately
before applying the command; output can advance it after the client predicts an
offset. Logs preserve that client prediction separately. Linux Xvfb proves
snapshot/queue behavior but does not guarantee
continuous frame delivery; sustained presentation latency needs a native host.
Xvfb runs with `-noreset` so a last-client disconnect cannot send a second
readiness signal during the wrapper's temporary-directory cleanup.

A local Linux comparison on an AMD Ryzen 5 5600GT used a flat Alacritty
baseline at
`d1fbff3` and this implementation, with the same alternating fixture. Median
elapsed times below are microseconds; processing and snapshot construction are
measured separately.

| Fixture | Flat Alacritty snapshot | Shared Alacritty process / snapshot | Ghostty process / snapshot |
| --- | ---: | ---: | ---: |
| ASCII with scrolling | 92.2 | 26.0 / 136.1 | 69.7 / 327.5 |
| Styled with scrolling | 97.9 | 24.3 / 136.2 | 82.5 / 327.1 |
| Unicode with scrolling | 95.2 | 23.9 / 137.3 | 77.9 / 328.2 |
| Sparse row changes | 92.6 | 0.2 / 3.8 | 0.2 / 8.9 |
| Full-screen rewrites | 92.8 | 26.1 / 101.5 | 9.0 / 323.0 |

The separate resize/reflow fixture alternates 100 and 120 columns at 40 rows.
Median resize/snapshot times were 4.8/91.4 microseconds for the flat baseline,
5.3/169.9 for shared Alacritty, and 19.2/342.6 for Ghostty. The fixture verifies
that a marker and 180 characters survive every reflow. Both benchmark tests run
serially. Combined processing-plus-snapshot intervals and processing throughput
are also reported by the tasks.

Both shared-row adapters reused 7,799 of 8,000 rows in the sparse fixture. The
scrolling fixtures retained 10,000 history rows with Alacritty and 15,704 with
Ghostty's byte budget. History is reported rather than presented as identical.
Shared rows cut sparse snapshot work substantially, but add allocation and
reference-counting costs when most rows change. Ghostty's full-screen parser
was faster in this sample while its snapshot conversion was slower. This
experiment does not establish an overall Ghostty speed advantage. Results are
local observations, not fixed performance guarantees; rerun on the same host
and native toolchain before drawing conclusions.

The Xvfb desktop scroll gate passed for both adapters with the original budgets.
Alacritty reported median/p95 snapshot times of 140/213 microseconds and p95
input-to-snapshot latency of 2,645 microseconds. Ghostty reported 278/363 and
2,343 microseconds respectively. Both stayed at one concurrent request and one
queued update. The flat baseline reported 92/138 and 2,191 microseconds. All
three renderer runs produced no usable paint samples; no displayed-frame-rate
comparison is claimed.
