# libghostty-vt-sys CPU backport

`libghostty-vt-sys-0.2.1` contains the published registry crate with the
12-line CPU-target fix from upstream commit
[`f720ad74a66e333181986fd5008f34c0a788a119`](https://github.com/Uzaaft/libghostty-rs/commit/f720ad74a66e333181986fd5008f34c0a788a119)
applied to `build.rs`, plus one local packaging correction. The CPU fix defaults
to `-Dcpu=baseline`, accepts a nonempty
`LIBGHOSTTY_VT_SYS_CPU` override, and tells Cargo to rebuild when it changes.
Huterm forces `baseline` in `.cargo/config.toml` for portable native artifacts.

The packaging correction changes `rerun-if-changed` to the crate-relative
`build.rs`. The published workspace-relative path does not exist in a vendored
crate and otherwise makes every Cargo invocation rerun the native build script.

The original crate archive SHA-256 is
`865fed12a8b2bba3507b3bccd0bef439e06ef1100e652fe6b71d132c41ee8db0`.
Its published VCS revision is `46a9d2ac941ed600cf43c5e6299c8dfd1d3a1ef0`.
All published files are retained unchanged except `build.rs`; Cargo's local
`.cargo-ok` extraction marker is omitted. `LICENSE-MIT` is copied from the
existing reviewed binding notice in `third-party/ghostty`. Huterm uses the MIT
option of the crate's `MIT OR Apache-2.0` license.

The safe wrapper remains registry version 0.2.1, native Ghostty remains
`a887df42c56f6de86c0fe6da9c4eeca37931e083`, and Zig remains 0.15.2. No current
upstream bindings or native API changes are included. The root Cargo patch
selects this local crate; its source identity invalidates old registry-built
native artifacts without deleting the rest of the Cargo cache.

Remove this directory and the Cargo patch when a reviewed published release
provides this CPU fix and is compatible with the selected native revision and
Zig toolchain. Update binding, native-source, and license pins together if that
release requires an API change. Keep the forced baseline CPU contract.
