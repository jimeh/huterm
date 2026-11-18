# Ghostty Zig packages

These are the Zig packages that the pinned Ghostty lib-vt build needs, so no
build depends on codeberg.org or deps.files.ghostty.org being reachable. Each
file is the tarball Zig writes to its global cache as `p/<hash>.tar.gz`, named
by the package hash that Ghostty's or a dependency's `build.zig.zon` pins.
`scripts/ghostty-source.json` lists each under `zig_packages` with that
declared upstream URL and the file's SHA-256.

`mise run ghostty:prepare` checks every SHA-256 and publishes the verified
files beside the prepared source. Before `zig build`, the `huterm-ghostty`
build script makes them the only packages in its private Zig cache,
`.native/ghostty-zig-cache`. Zig then checks each tarball's contents against
the pinned package hash before using it, so a changed file fails the build
instead of changing what it compiles.

Regenerate this directory with `mise run ghostty:zig-packages` after a Ghostty
pin or Zig bump. It fetches from the upstream hosts into an empty cache, and
Zig's compression is deterministic, so a rerun on an unchanged pin produces
identical files. See
[vendored Zig packages](../../docs/agents/terminal-engines.md#vendored-zig-packages).

Every package keeps its upstream license file inside its tarball: aro, MIT;
translate-c, MIT; uucode, MIT; Highway, Apache-2.0 or BSD-3-Clause; Wuffs,
Apache-2.0 or MIT; `pixels`, CC0-1.0; zlib, the zlib license. zlib's tarball
also carries contributed sources under other licenses, which Zig's package
hash covers and which no build compiles: `contrib/ada` under GPL-2.0-or-later
with the GNAT linking exception, and `contrib/dotzlib` under BSL-1.0. These
files are build inputs and do not ship. The notices for the code the VT library
links, including uucode, Highway, and Wuffs, are in
[the Ghostty notices](../ghostty/README.md).
