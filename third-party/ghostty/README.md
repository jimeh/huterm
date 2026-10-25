# Ghostty VT notices

These notices accompany the statically linked Ghostty VT engine.
`provenance.json` records the origin and checksum of each notice.

Huterm's own `huterm-ghostty` crate binds the C API directly. The native source
is Ghostty `cec8b92f1ecd176a20a56aa405021e7ae991b996`, built with Zig 0.17.0.

The VT build includes Ghostty, uucode 0.2.0 at
ea62149739404a73c202b48a33bf6dd2af4bd9b0 (including Unicode and UTF-8 helper
notices), simdutf 9.0.0, Highway 66486a10623fa0d72fe91260f96c892e41aceb06, and
Wuffs 0.4 from the wuffs-mirror-release-c revision
7411f488fe2e2c205c3d3b3d28638b7356522930.
The vendored simdutf header identifies version 9.0.0; its package manifest's
5.2.8 label is stale. MIT is selected where an upstream dual license permits it.
Both Highway notices are retained for its Apache-2.0/BSD-3-Clause source files.
Ghostty compiles Wuffs into the VT library whenever its Kitty graphics support
is built. Huterm disables Kitty graphics at run time, but the code still links.

The upstream archive and Zig dependency evaluation include application-related
sources that are not linked into the VT library. This notice set describes the
VT build, not every optional application dependency. Zig may download additional
lazy dependencies while evaluating the build; their hashes remain pinned in the
upstream package manifests. No native Ghostty renderer or font stack is linked.
Huterm's Rust terminal graphics port has [separate notices](../terminal-graphics/README.md).

The `huterm-ghostty` build script builds libghostty-vt from a private copy of
the verified source, after applying the reviewed patches that
`scripts/ghostty-source.json` lists, and links the static archive, which needs
only libc. Cargo forces `HUTERM_GHOSTTY_OPTIMIZE=ReleaseFast` and a portable
`HUTERM_GHOSTTY_CPU=baseline` target. Record CPU and optimization when
comparing measurements. The crate's generated FFI declarations derive from the
MIT licensed Ghostty headers covered by `ghostty-MIT.txt`.
