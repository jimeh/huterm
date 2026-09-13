# Ghostty VT notices

These notices accompany the statically linked Ghostty VT engine.
`provenance.json` records the origin and checksum of each notice.

Both Rust wrappers use published 0.2.1 archives with the matched upstream
revision `5988a0b78b4aa804d1c12e66bbfe662bd97d81c0` applied through the
[reviewed patch stack](../vendor/README.md#matched-libghostty-upstream-upgrade).
The native source is Ghostty `22d13172cde98a0a4dda05d3d6a3fcb0dd8ed018`, built
with Zig 0.16.0.

The VT build includes Ghostty, uucode 0.2.0 at
2826a37a4562284fdacd8fa029d49509cc9bffcd (including Unicode and UTF-8 helper
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

The sys crate selects static linking and maps release builds to ReleaseFast.
The upstream sys crate supplies a CPU option; Huterm forces a portable baseline
target. See [the patch provenance](../vendor/README.md).
Record CPU and optimization when comparing measurements.
