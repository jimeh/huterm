# Ghostty VT notices

These notices accompany the statically linked Ghostty VT engine.
`provenance.json` records the origin and checksum of each notice.

Huterm's own `huterm-ghostty` crate binds the C API directly. The native source
is Ghostty `56dbc4a768778753737a3b9cbe0a3f9b4e434553`, built with Zig 0.16.0.

The VT build includes Ghostty, uucode 0.2.0 at
9d55524551411b493cca41ca06363625d90aff1e (including Unicode and UTF-8 helper
notices), simdutf 9.0.0, and Highway 66486a10623fa0d72fe91260f96c892e41aceb06.
The vendored simdutf header identifies version 9.0.0; its package manifest's
5.2.8 label is stale. MIT is selected where an upstream dual license permits it.
Both Highway notices are retained for its Apache-2.0/BSD-3-Clause source files.

The upstream archive and Zig dependency evaluation include application-related
sources that are not linked into the VT library. This notice set describes the
VT build, not every optional application dependency. Zig may download additional
lazy dependencies while evaluating the build; their hashes remain pinned in the
upstream package manifests. No native Ghostty renderer or font stack is linked.
Huterm's Rust terminal graphics port has [separate notices](../terminal-graphics/README.md).

The `huterm-ghostty` build script builds libghostty-vt from a private copy of
the verified source and links the static archive, which needs only libc. Cargo
forces `HUTERM_GHOSTTY_OPTIMIZE=ReleaseFast` and a portable
`HUTERM_GHOSTTY_CPU=baseline` target. Record CPU and optimization when comparing
measurements. The crate's generated FFI declarations derive from the MIT
licensed Ghostty headers covered by `ghostty-MIT.txt`.
