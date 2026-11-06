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

Mise 2026.10.19 and newer default to `lockfile_mode = "generate"`, so
`mise install` downloads each platform in `mise.lock` that lacks a checksum,
Windows included, and fails when that platform's URL does not resolve. Make
each `http:` tool's URL template resolve on every locked platform, as
elvish's `os()`-selected archive extension does. CI never regenerates the lock:
the Mise action adds `--locked` whenever `mise.lock` exists, and every other CI
install passes it explicitly. Keep `--locked` on any new CI install.

The dependency graph needs Rust 1.95 or newer: `gpui-pre` 0.3.11 uses
`std::hint::cold_path`, and libghostty-vt 0.2.1 requires 1.90. Project tooling
pins Rust 1.99.

On macOS, GPUI shader compilation needs Xcode's optional Metal Toolchain;
`mise run doctor` checks it and reports the installation command.

On Ubuntu, GPUI's X11 backend needs both XKB development packages at link time
and a Vulkan device at runtime; CI uses Mesa's software Vulkan driver under
Xvfb.

## Cargo release age

Cargo has no release-age setting, so `cargo update` accepts releases inside
the three-day window. After updating, compare each changed version's
`created_at` from `https://crates.io/api/v1/crates/<name>/<version>` with a
72-hour cutoff, and roll back newer ones with
`cargo update -p <name> --precise <previous>`. Check the crates that a
`gpui-pre` snapshot pulls in the same way.

## Bumps that change generated files

Some dependencies shape committed generated files, and Dependabot cannot
regenerate them. Their bump pull requests fail CI until a follow-up commit on
the same branch runs the generator:

- `bindgen`: `mise run ghostty:bindings`, checked by
  `ghostty:bindings:check` on macOS.
- `schemars`: `mise run schema:generate`, checked by `schema:check`.
- `lucide-static`: `mise run ui-icons:generate`, checked by
  `ui-icons.test.ts`.
- Any Cargo crate that ships, and the Rust toolchain:
  `mise run licenses:generate`, checked by `licenses:check`. A toolchain bump
  also needs fresh `third-party/licenses/rust/` texts and provenance, and a
  crate that ships no license file needs a clarification; see
  [the notices README](../../third-party/licenses/README.md).

`gpui-pre` crates are ignored by Dependabot and upgrade through the vendor
workflow below. So are the font crates `huterm-gpui` pins to GPUI's versions:
`zed-font-kit`, `core-foundation`, `core-text`, and `fontdb`. A GPUI upgrade
moves those pins to the versions it resolves, along with the `gpui-pre-apple`
and `gpui-pre-wgpu` dev-dependencies that the font registration tests use.
A bump that changes lint behavior, such as `syn`, may also need code changes
before Clippy passes. AccessKit's macOS adapter, `accesskit_macos`, is vendored
at the version GPUI resolves with Huterm's terminal accessibility patches, so a
GPUI upgrade that moves AccessKit migrates those patches too.

## Bundled terminal font

DejaVu Sans Mono 2.37's four unmodified faces live in
`crates/huterm-gpui/assets/fonts/dejavu/`. To bump them, fetch the official
release archive, verify its origin and SHA-256, and copy the Book, Bold,
Oblique, and Bold Oblique TTFs without rewriting them. Refresh the upstream
`third-party/dejavu/LICENSE`, README, and every file hash and URL in
`provenance.json`, plus the component version in
`third-party/licenses/components.json`. The component records Bitstream Vera,
Arev, and the public-domain DejaVu changes; keep the full upstream notice.
Run `mise run licenses:generate`, `mise run licenses:check`, the
`terminal_font` tests (native registration and weights on both platforms),
`mise run smoke:renderer`, and the platform package checks. The notices ship
as resources; the fonts themselves are embedded and registered before lookup.

## GPUI and nix

GPUI comes from `gpui-pre`, huacnlee's crates.io snapshots of Zed's main
branch, because Zed has not published GPUI since 0.2.2. Depend on it as
`gpui = { package = "gpui-pre" }` so code keeps the `gpui` crate name, and on
`gpui-pre-platform` (`gpui_platform::application()`) for the native
`Application`. Its crates pin each other exactly, so upgrade them together and
review every vendored patch against the new Zed revision. Upstream's Linux
font-kit links fontconfig at build time; the direct `zed-font-kit` dependency in
`huterm-gpui` restores `source-fontconfig-dlopen` to avoid a build-time link
and packaged dependency on fontconfig. This feature does not itself load the
library. Never invoke font-kit's native Linux source: dlopening libfontconfig
also loads system FreeType, which package verification rejects. Linux font
queries for Settings use the pinned fontdb database. Glyph checks use
font-kit's statically linked FreeType loader.

Enable GPUI's `test-support` feature only through `huterm-gpui`'s own
`test-support` feature, never as a dev-dependency. Cargo builds examples with
dev-dependencies, and the smoke and benchmark binaries are examples, so a
dev-dependency would compile GPUI's leak detection into them. The feature adds
`proptest` to `Cargo.lock` and sets GPUI's `wayland` cfg on Linux; the Linux
container builds and runs it without extra packages. `huterm-ui` has its own
`test-support` feature, which `huterm-gpui`'s enables, so the workspace tasks
compile both crates' test-gated code through feature unification.

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

The three `native-key-identity` GPUI patches retain native layout characters,
raw Shift, and physical US keys separately from binding equality for Kitty
keyboard input. Core GPUI carries the optional identity to interceptors;
macOS and Linux populate it before normalization. Drop them when a reviewed
snapshot supplies equivalent native identity. The vendor README records the
platform derivation and required input-smoke checks.

The `gpui-pre-macos` `vm-display-link` patch keeps `CVDisplayLink` frame
pacing when `kern.hv_vmm_present` reports a virtual machine, where GPUI's
`CAMetalDisplayLink` stops issuing updates. Drop it when a reviewed snapshot
delivers Metal-link frames on GitHub's macOS runners; CI's macOS smokes are the
check. [Window presentation](window-presentation.md) records the evidence.

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
