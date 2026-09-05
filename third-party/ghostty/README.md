# Ghostty VT notices

These notices accompany the optional, statically linked Ghostty VT engine.
`provenance.json` records the origin and checksum of each notice.

The Rust wrappers are libghostty-vt and libghostty-vt-sys 0.2.1. Their source
revision is 46a9d2ac941ed600cf43c5e6299c8dfd1d3a1ef0. The native source is Ghostty
a887df42c56f6de86c0fe6da9c4eeca37931e083, built with Zig 0.15.2.

The VT build includes Ghostty, uucode 0.2.0 (including Unicode and UTF-8 helper
notices), simdutf 9.0.0, and Highway 66486a10623fa0d72fe91260f96c892e41aceb06.
The vendored simdutf header identifies version 9.0.0; its package manifest's
5.2.8 label is stale. MIT is selected where an upstream dual license permits it.
Both Highway notices are retained for its Apache-2.0/BSD-3-Clause source files.

The upstream archive and Zig dependency evaluation include application-related
sources that are not linked into the VT library. This notice set describes the
VT build, not every optional application dependency. Zig may download additional
lazy dependencies while evaluating the build; their hashes remain pinned in the
upstream package manifests. No Ghostty renderer or font stack is linked.

The sys crate selects static linking and maps release builds to ReleaseFast.
This published wrapper has no CPU override. Linux builds use Zig's native CPU
default; native macOS builds use Ghostty's upstream baseline CPU workaround.
Record CPU and optimization when comparing measurements.
