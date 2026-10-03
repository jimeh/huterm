# Dependencies and vendored crates

Read this before changing Rust dependencies, the toolchain, GPUI, or a vendored
crate. The release-age policy and license rules are in
[AGENTS.md](../../AGENTS.md#dependency-changes).

## Toolchain and platform requirements

Keep the Rust version, minimal profile, and `clippy`/`rustfmt` components in
`mise.toml` aligned with `rust-toolchain.toml`; CI installs only the named Mise
tools for each job. `mise.lock` also records the Rust version, so run
`mise install` and commit the lock after any toolchain bump; otherwise CI
fails to resolve the tool. `scripts/policy-alignment.test.ts` checks that the
three files agree.

The dependency graph needs Rust 1.95 or newer: `gpui-pre` 0.3.6 uses
`std::hint::cold_path`, and libghostty-vt 0.2.1 requires 1.90. Project tooling
pins Rust 1.98.

On macOS, GPUI shader compilation needs Xcode's optional Metal Toolchain;
`mise run doctor` checks it and reports the installation command.

On Ubuntu, GPUI's X11 backend needs both XKB development packages at link time
and a Vulkan device at runtime; CI uses Mesa's software Vulkan driver under
Xvfb.

## Bumps that change generated files

Some dependencies shape committed generated files, and Dependabot cannot
regenerate them. Their bump pull requests fail CI until a follow-up commit on
the same branch runs the generator:

- `bindgen`: `mise run ghostty:bindings`, checked by
  `ghostty:bindings:check` on macOS.
- `schemars`: `mise run schema:generate`, checked by `schema:check`.
- `lucide-static`: `mise run ui-icons:generate`, checked by
  `ui-icons.test.ts`.

`gpui-pre` crates are ignored by Dependabot and upgrade through the vendor
workflow below. A bump that changes lint behavior, such as `syn`, may also
need code changes before Clippy passes.

## GPUI and nix

GPUI comes from `gpui-pre`, huacnlee's crates.io snapshots of Zed's main
branch, because Zed has not published GPUI since 0.2.2. Depend on it as
`gpui = { package = "gpui-pre" }` so code keeps the `gpui` crate name, and on
`gpui-pre-platform` (`gpui_platform::application()`) for the native
`Application`. Its crates pin each other exactly, so upgrade them together and
review every vendored patch against the new Zed revision. Upstream's Linux
font-kit links fontconfig at build time; the direct `zed-font-kit` dependency in
`huterm-gpui` restores `source-fontconfig-dlopen` so packages keep loading
fontconfig at runtime. Zed's HTTP client pulls `libbz2-rs-sys`, allowed through
a crate-scoped cargo-deny license exception.

Keep direct `nix` on the newest 0.29.x release while `portable-pty`'s
`MasterPty` exposes only `as_raw_fd()`. Nix 0.30 and newer require `AsFd` for
`fcntl`; upgrading earlier would require an unsafe borrowed-descriptor
conversion in `huterm-core`, which denies unsafe code. Remove the matching
Dependabot ignore when a `portable-pty` release exposes a safe borrowed
descriptor.

## Vendored crates

Patched registry crates use ordered named patches in
`third-party/vendor/sources.json` against checksum-pinned release archives. Agents
own patch maintenance for authorized fixes; follow
`third-party/vendor/README.md` without asking the user to operate the workflow.
Run `vendor:status`, then `mise run vendor:start -- <crate> <patch>` before edits.
Edit and test the fully applied vendor tree normally, then run `vendor:finish`.
Resolve replay conflicts in the printed private workspace and use `vendor:continue`.
Use `vendor:reopen` for more build-tree edits after finish starts. Only explicitly
adopt pre-existing edits after reviewing their scope; never absorb unexplained
source drift or edit recipe files during an active session. Review patch ownership,
run `vendor:check` and the affected behavioral checks, and finish every session
before handoff or commit. Commit source and patches together when authorized.

`gpui-pre` archives carry no Cargo VCS file; their manifest entries set
`"snapshot": "gpui-pre"` so the checker reads the Zed crate and revision from
`package.metadata.gpui-pre`. The archive remains the exact source baseline.
Normal builds use the vendored tree directly.

Private Git snapshots must force-add ignored package files and disable attributes
that transform bytes or omit archive entries; otherwise the patch recipe can lose
published files or alter line endings.

Disable automatic Git maintenance in private vendor session repositories. On
Git 2.55, detached maintenance recreated a deleted GPUI session directory after
finish, leaving metadata without state.json and blocking subsequent commands.

Set Git's discovery ceiling to the parent of each vendor command's working
directory. Scratch patch verification under `.native/vendor` otherwise discovers
Huterm's repository and `git apply` silently skips paths outside that subdirectory.
Private session repositories still resolve their own `.git` directory normally.

Read vendor regular files through one no-follow descriptor and compare its
identity with the tree entry before hashing. Archive extraction must consume the
same buffer that passed checksum verification; checking a path and reopening it
allows a concurrent replacement to bypass the checksum.

Preserve upstream formatting in vendored crates. The staged Rust formatter
excludes `third-party/vendor`; Cargo still compiles it as a dependency.

A path-imported vendor test module still gets traversed by cargo fmt even when
its crate is excluded from the workspace. Keep rustfmt::skip on that module
import so focused production-helper tests preserve upstream vendor formatting.
