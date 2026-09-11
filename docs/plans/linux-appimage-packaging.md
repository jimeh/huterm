# Linux AppImage and binary tarball plan

Status: proposed. Written on 2026-09-10 after investigation and agreement on
the packaging direction.

## Outcome

Publish native Linux desktop builds for x86_64 and aarch64 alongside the
existing universal macOS archive. Each Linux architecture gets two artifacts:

| Architecture | AppImage | Binary tarball |
| --- | --- | --- |
| x86_64 | `Huterm-<version>-Linux-x86_64.AppImage` | `Huterm-<version>-Linux-x86_64.tar.gz` |
| aarch64 | `Huterm-<version>-Linux-aarch64.AppImage` | `Huterm-<version>-Linux-aarch64.tar.gz` |

Both formats contain the same neutral application payload. The tarball does
not contain `AppRun`, `.DirIcon`, an AppImage runtime, or other AppImage-only
files. The AppImage build copies the neutral payload into an AppDir, adds those
files, and creates the final filesystem image.

Keep Huterm's application code and native terminal engines linked as they are
today. Do not pursue a fully static Linux executable. Bundle only the small set
of shared libraries that are safe to keep private. Leave the C runtime, XCB
core, Vulkan loader, and graphics drivers under host ownership.

The initial Linux release contract is X11 or XWayland plus Vulkan. Native
Wayland support remains separate work because Huterm currently enables GPUI's
X11 backend only.

## Settled decisions

- Build x86_64 and aarch64 on native GitHub-hosted Linux runners. Do not use
  emulation for release evidence.
- Build release artifacts on Ubuntu 22.04 to set a glibc 2.35 compatibility
  ceiling. Fail verification if the result imports a newer required GLIBC
  symbol.
- Produce one neutral directory before creating either public format.
- Use a repository-owned Bun packaging script and discoverable Mise tasks.
  Keep Linux packaging independent from the macOS-only cargo-packager
  configuration.
- Use tagged AppImage tooling with committed SHA-256 digests. Supply a pinned
  type-2 runtime explicitly so appimagetool cannot fetch a moving runtime.
- Bundle the xkbcommon family only after the package verifier proves the exact
  dependency closure. Keep every low-level or hardware-dependent library on an
  explicit exclusion list.
- Require the Linux release executable to contain no dynamic FreeType
  dependency. Ship the FreeType License notice for the C source compiled by
  `freetype-sys` when the system library does not meet its selection threshold.
- Give bundled libraries relative ELF runpaths. Do not set a process-wide
  `LD_LIBRARY_PATH`, which could affect Vulkan driver loading.
- Publish one `SHA256SUMS` covering the macOS archive, four Linux artifacts,
  and both schema files.
- Keep GitHub Release publication as the final operation. Any earlier failure
  leaves the release draft intact.

## Scope and non-goals

This work includes Linux package construction, package verification, native
architecture CI, release integration, user-facing installation notes, and the
licence notices required by redistributed libraries.

It does not add:

- deb, RPM, Flatpak, Snap, or distribution repositories;
- native Wayland support;
- a universal or multi-architecture Linux executable;
- automatic desktop installation from the tarball;
- AppImage update metadata or a separate updater;
- GPG signing for AppImages;
- glibc, XCB core, Vulkan, Mesa, proprietary drivers, or system fonts inside
  the application bundle; or
- additional static linking solely to reduce the number of files in the
  archive.

GitHub release digests and the published `SHA256SUMS` are the initial integrity
mechanism. AppImage signing can be considered later if Huterm gains a key
distribution and rotation policy.

## Current state and constraints

The root package metadata is specific to the universal macOS app. It selects
the `app` format, the universal Apple target, macOS resources, and the macOS
build task. Linux packaging should not add conditional policy to that block.

The Linux GPUI build enables `font-kit` and `x11`. The application and both
terminal engines compile into the Huterm executable. An inspected x86_64
release build had these direct ELF dependencies:

```text
libxcb.so.1
libxkbcommon.so.0
libxkbcommon-x11.so.0
libgcc_s.so.1
libm.so.6
libc.so.6
ld-linux-x86-64.so.2
```

A real Xvfb launch also loaded the host Vulkan loader and Mesa driver modules.
It did not load dynamic FreeType or Fontconfig libraries. The Ubuntu 24.04
build imported required GLIBC symbols through 2.35 and two weak GLIBC 2.39
pidfd symbols. Building on Ubuntu 22.04 and auditing the final dynamic symbol
table provides a clear, enforceable compatibility boundary.

The existing Linux Docker helper accepts `amd64` and `arm64`, but non-native
execution depends on host binfmt registration. The local arm64 probe failed
before compilation because the Docker host did not have ARM emulation set up.
Release validation must use native aarch64 runners instead.

The current release workflow builds, signs, notarizes, uploads, verifies, and
publishes from one macOS job. Adding Linux requires platform build jobs and a
separate final job that owns the complete release asset set.

## Artifact contract

### Neutral bundle

The packaging script first creates this directory:

```text
Huterm-<version>-Linux-<arch>/
├── bin/
│   └── huterm
├── lib/
│   └── huterm/
│       └── <approved private shared libraries>
├── share/
│   ├── applications/
│   │   └── app.huterm.dev.desktop
│   ├── icons/hicolor/512x512/apps/
│   │   └── app.huterm.dev.png
│   ├── metainfo/
│   │   └── app.huterm.dev.metainfo.xml
│   ├── huterm/
│   │   └── package-manifest.json
│   └── licenses/huterm/
│       ├── <Huterm and bundled dependency notices>
│       ├── <built-in theme notices>
│       ├── <Ghostty and terminal-graphics notices>
│       └── <FreeType License notice>
└── README.md
```

`bin/huterm` is the actual ELF executable. Set its runpath to
`$ORIGIN/../lib/huterm`. Set each bundled library's runpath to `$ORIGIN` when
it has a private dependency in the same directory. The verifier rejects an
absolute runpath, build directory, empty runpath component, or reference that
escapes the bundle.

The bundle README states the remaining system contract, how to run Huterm, and
how to copy the desktop file and icon for per-user integration. Extracting the
archive must never write outside its top-level directory.

The packaging script requires `SOURCE_DATE_EPOCH`. Release jobs derive it from
the exact release commit time. Local Git builds use the checked-out HEAD; the
Linux Docker helper passes the source commit time into its Git-free workspace.
Fail with an actionable error when none of those sources is available.

Normalize file order, ownership, modes, and timestamps from that value. The
top-level directory and its children use root ownership metadata in the tar
stream without requiring root during creation. Compression must omit timestamps
and host-specific metadata.

### Desktop metadata

Add committed Linux desktop and AppStream metadata under `assets/linux/`.
Generate and commit a 512-pixel Linux icon through the existing icon workflow,
then validate all three files during normal checks. The hicolor index does not
declare a 1024-pixel application directory, so do not install the existing
1024-pixel source PNG there.

The desktop entry uses:

- application ID `app.huterm.dev`;
- display name `Huterm`;
- `Exec=huterm` in the installed form;
- `Icon=app.huterm.dev`;
- terminal mode disabled because Huterm creates its own window; and
- the development application category.

The AppDir copy may adjust `Exec` only if AppImage tooling requires it. Keep
the committed source valid as ordinary freedesktop metadata rather than
encoding an AppImage path in it. Huterm already sets its X11 class to
`app.huterm.dev`, matching the desktop filename. Omit `StartupWMClass` unless
desktop integration testing proves that a supported environment needs it.

### Tarball

Create the tarball directly from the neutral directory. Its file list must not
contain AppImage root symlinks, `AppRun`, `.DirIcon`, a type-2 runtime, or a
SquashFS image. Running the extracted `bin/huterm` must resolve approved private
libraries relative to the executable and must not depend on the extraction
location.

### AppImage

Create a temporary AppDir by copying the neutral bundle directories under
`usr/`. Add:

- `AppRun` as a relative symlink to `usr/bin/huterm`, avoiding a shell runtime
  dependency and preserving the executable's relative runpath;
- the required root desktop file, matching its neutral-payload source plus
  appimagetool's `X-AppImage-Version` field;
- the required root icon symlink;
- `.DirIcon`; and
- the architecture-matched type-2 runtime during image creation.

Run the architecture-matched appimagetool with an explicit runtime file and
explicit output path. Set the AppImage architecture and version environment
values rather than relying on filename inference. Do not add update information
in this slice.

The package verifier extracts the finished AppImage and compares the neutral
payload byte for byte, allowing only the declared AppImage-specific additions.

## Dynamic library policy

The packaging script owns two explicit lists. An allowlist names libraries that
may be copied into `lib/huterm`; an exclusion list names dependencies that must
resolve from the host. Any dependency in neither list fails the build. This
turns dependency changes into a reviewable packaging decision instead of
silently expanding the artifact.

Start with this candidate private set, then confirm it against the Ubuntu 22.04
closure on both architectures:

```text
libxkbcommon.so.0
libxkbcommon-x11.so.0
libxcb-xkb.so.1
```

Keep these classes host-owned:

- glibc, `libm`, the ELF loader, and name-service libraries;
- `libgcc_s.so.1`;
- `libxcb.so.1`, `libXau`, `libXdmcp`, and XCB transport support;
- the Vulkan loader, ICD files, Mesa libraries, and vendor GPU drivers;
- Fontconfig, system font libraries, system fonts, locale data, and XKB data;
  and
- libraries on the maintained AppImage exclusion list.

If the candidate xkbcommon closure pulls in a host-owned library, retain that
dependency as host-owned. Do not copy it merely to make the package verifier
green. If privately bundling xkbcommon causes a cross-distribution failure,
drop the private copies and document the runtime package requirement. Do not
respond by statically linking XCB or bundling more of the desktop stack.

Record an upstream licence and source package for every private shared library.
The normal licence check must cover this manifest because Cargo's audit cannot
see copied ELF files.

FreeType needs a separate guard. `freetype-sys` 0.20.1 links a system FreeType
only when pkg-config reports at least 24.3.18; otherwise it compiles its bundled
C source. Ubuntu 22.04 takes the bundled path. Assert that `libfreetype.so.6`
is absent from `DT_NEEDED`, commit the FreeType License text used for that
static code, and include it in every Linux bundle. A future build-image change
must fail instead of silently changing FreeType ownership.

The bundle's licence directory also contains the six built-in theme notices,
Ghostty's notice, and the terminal-graphics notice already required by macOS
packaging. Verify those exact files rather than limiting the check to copied
shared libraries.

## Packaging implementation

Add a focused Bun module for Linux packaging and unit-test its pure policy and
manifest logic. The script should provide commands equivalent to:

```text
build             build the native release executable and both artifacts
verify-bundle     verify one neutral or extracted bundle
verify-artifacts  verify the AppImage and tarball as a pair
```

The command accepts an expected version and architecture but derives the actual
architecture from the ELF header. It rejects a mismatch. Normalize `amd64` and
`x86_64` to `x86_64`; normalize `arm64` and `aarch64` to `aarch64` only at input
boundaries.

Expose durable workflows through Mise:

```text
mise run package:linux
mise run package:linux:verify
```

`package:linux` requires Linux, prepares the pinned Ghostty source, verifies
icons, performs the locked release build, creates the neutral bundle, and
produces both public artifacts for the native architecture.
`package:linux:verify` accepts already-built artifacts and performs no network
access.

Store AppImage tool names, release URLs, versions, architecture mappings, and
SHA-256 digests in a committed manifest. Download to a cache outside `dist`,
verify the digest before execution, and never fall back to a mutable release
URL. Apply the repository's three-day release-age policy when selecting or
updating these tools.

Write `share/huterm/package-manifest.json` from observed build inputs. For each
private ELF file, record the SONAME, SHA-256 digest, Debian source and binary
package names, and exact installed package version from `dpkg-query`. Record
the build architecture, release commit, `SOURCE_DATE_EPOCH`, and packaging tool
versions too. Verification checks the manifest against the staged bytes. This
makes an apt update visible in release evidence even though Ubuntu's update
repository is not immutable.

Use temporary directories for intermediate bundles and AppDirs. Publish into
`dist/linux/<arch>/` only after both formats pass verification. A failed build
must not leave an artifact with the final public name.

Do not use the root cargo-packager block for Linux. Its current configuration
correctly expresses the macOS universal application, while the Linux pipeline
needs two native architectures, a shared tarball payload, explicit dynamic
library policy, and pinned AppImage runtime selection.

Install the release builder prerequisites explicitly on both Ubuntu 22.04
architectures. The required set starts with the existing Linux build and smoke
packages and adds packaging validators:

```text
build-essential ca-certificates clang cmake curl file git locales openbox
patchelf pax-utils pkg-config python3 ripgrep rsync xauth xcompmgr xdotool
xvfb xz-utils zsh libxkbcommon-dev libxkbcommon-x11-dev
mesa-vulkan-drivers fontconfig fonts-dejavu-core x11-xkb-utils x11-utils
desktop-file-utils appstream
```

Keep the package list identical across architectures unless Ubuntu uses a
different package name. Route every native Cargo build through
`bash scripts/build-exec.sh`, including Linux packaging, so the existing
Ghostty build wrapper remains the single build entry point.

## Native architecture CI

Treat aarch64 as a supported Linux platform rather than only a release target.
Extend the Linux check and smoke matrices to include a native ARM runner. Keep
platform-independent policy work single-run.

Use explicit runner labels:

| CI role | x86_64 | aarch64 |
| --- | --- | --- |
| Linux checks and tests | `ubuntu-24.04` | `ubuntu-24.04-arm` |
| Linux desktop smoke | `ubuntu-24.04` | `ubuntu-24.04-arm` |
| Release packaging | `ubuntu-22.04` | `ubuntu-22.04-arm` |

Every Linux job records `uname -m`, `RUNNER_ARCH`, image metadata, and the exact
Git commit in its existing evidence directory. Assert that x86_64 jobs run on
x86_64 and ARM jobs run on aarch64 before installing or compiling native
dependencies.

Keep the existing required check contexts `Verify Linux x86_64` and
`Verify macOS arm64`. Adding ARM legs to the existing `checks` and `smoke` job
IDs makes both aggregate gates require those legs without a repository-ruleset
change. Update the workflow comments to state that the historical context names
cover the full platform matrix. Creating a separate required
`Verify Linux aarch64` context would need an independently authorized ruleset
change and is outside this implementation.

Include `${{ runner.arch }}` in every manual rust-cache key used by an
architecture matrix, including the smoke cache that disables the automatic
Rust environment key. Do not rely on undocumented action key composition to
separate x86_64 and aarch64 artifacts.

The native ARM smoke runs the same renderer, desktop, input, integration,
fullscreen, and quake checks as x86_64 unless a test has a documented
architecture-specific limitation. Do not replace native ARM execution with a
successful cross-compile.

## Release workflow

Refactor the reusable release workflow into four responsibilities:

1. A preflight job validates the exact SHA, version, ancestry rules, and draft
   release identity before any platform build. Publishing mode may read the
   draft with the existing contents-write bot token. Manual non-publishing mode
   keeps the existing exact-branch-SHA exception and does not inspect a tag or
   GitHub Release.
2. The macOS job builds the signed, notarized, and stapled universal archive.
   It uploads that verified payload as an Actions artifact but does not upload
   to the GitHub Release or publish it.
3. Separate native Linux x86_64 and aarch64 jobs build and verify each AppImage
   and tarball pair. Each job uploads its two verified payloads plus a
   per-platform digest manifest as an immutable, attempt-qualified Actions
   artifact, exposes that exact name to downstream jobs, and receives no
   release credential.
4. A final assembly job downloads every platform artifact, checks out the exact
   release SHA for the committed schemas, validates the complete expected file
   set, verifies every payload against the digest manifest produced by its
   platform builder, creates `SHA256SUMS`, and rechecks the final inventory.
   Download every Actions artifact by the exact producer output, allowing a
   partial rerun to reuse an earlier successful producer without guessing the
   assembly attempt number.
   In non-publishing mode the job uploads the complete set as one short-lived
   Actions artifact. In publishing mode it mints a fresh release token, uploads
   the exact set to the validated draft, verifies remote names, sizes, states,
   and digests, then publishes the draft.

The final expected payload set is:

```text
Huterm-<version>-macOS-universal.zip
Huterm-<version>-Linux-x86_64.AppImage
Huterm-<version>-Linux-x86_64.tar.gz
Huterm-<version>-Linux-aarch64.AppImage
Huterm-<version>-Linux-aarch64.tar.gz
huterm.schema.json
huterm-theme.schema.json
SHA256SUMS
```

Move common asset naming, local inventory validation, checksum generation,
remote digest validation, draft validation, and publication out of the
macOS-specific script. Keep signing, notarization, stapling, Gatekeeper, and
macOS bundle verification in `release-macos.ts`. Linux package code must not
gain access to Apple or GitHub release credentials.

All platform jobs and the assembly job validate the checked-out SHA and Cargo
versions before running target-controlled build code. The assembly job refuses
missing, extra, duplicate, empty, or altered assets. Preserve the existing rule
that the remote draft must contain exactly the expected inventory before
publication.

Rewrite the existing release tests around the new job ownership. Tests that
currently inspect `jobs.release.steps`, macOS-only asset names, schema paths,
and the single build-and-publish job must target the preflight, macOS, Linux,
and assembly jobs instead. Move pure inventory assertions into the common
release module rather than keeping stale structural expectations in
`release-macos.test.ts`.

Update the release guide with the new artifact matrix, Linux system contract,
manual verification behavior, and first-release native checks.

## Implementation sequence

### 1. Define metadata and dependency policy

Add the desktop entry, AppStream metadata, neutral bundle layout, AppImage tool
manifest, private-library allowlist, host-library exclusion list, and bundled
licence manifest. Add validation for identifiers, architecture names, tool
digests, and dependency-policy overlap.

Add `aarch64-unknown-linux-gnu` to `deny.toml`'s audited targets so Cargo policy
covers the new supported platform rather than inferring it from macOS ARM.

Unit-test rejection of an unknown dependency, a library in both lists, a wrong
architecture, an unsafe runpath, an unverified tool, and missing licence data.
Run the desktop and AppStream validators on the committed metadata.

### 2. Build and verify the neutral bundle

Implement the native release build and neutral staging directory. Copy only the
approved xkbcommon closure, set relative runpaths, and create the reproducible
tarball.

Verify ELF architecture, executable permissions, GLIBC imports, exact runpaths,
dependency resolution, the absence of dynamic FreeType, licences, package
provenance, archive paths, normalized metadata, and the absence of
AppImage-only files. Capture the loaded library paths from `/proc/<pid>/maps`
during the package smoke so the host-owned Vulkan path and absence of dynamic
Fontconfig and FreeType remain durable evidence. Extract the tarball into a
fresh temporary directory and run existing Linux input coverage against its
`bin/huterm`; that smoke already exercises both Alacritty and Ghostty through
real PTYs.

### 3. Add the AppImage envelope

Transform the verified neutral bundle into an AppDir and build the AppImage
with the pinned appimagetool and type-2 runtime. Extract the result without
FUSE, compare its neutral payload with the tarball payload, and run the same
two-engine input smoke through `APPIMAGE_EXTRACT_AND_RUN=1`.

Add one manual FUSE-backed launch before the first public release. CI extraction
proves the application payload and fallback path but does not prove that a host
permits the runtime to mount the image.

### 4. Prove native aarch64 support

Add native ARM Linux checks and desktop smokes. Run the full package task on
`ubuntu-22.04-arm`, assert an aarch64 ELF and AppImage runtime, and retain the
package evidence as an Actions artifact. Resolve source or tool failures on the
native runner; do not enable binfmt in release jobs as a workaround.

### 5. Integrate the release asset set

Separate shared release inventory and publication logic from macOS signing.
Add the explicit Linux architecture jobs and final assembly job, then extend
script and workflow tests for the eight-file release contract. Preserve
non-publishing manual verification and draft retention on every failure.

### 6. Document and validate compatibility

Document the artifact choices, remaining runtime requirements, manual tarball
desktop integration, executable permission recovery, and the AppImage
extract-and-run fallback.

Before the first release, run both formats on Ubuntu 22.04 and Ubuntu 24.04 on
both native architectures. Run at least the x86_64 artifacts on a current
Fedora desktop or pinned Fedora container with X11, Vulkan software rendering,
and system fonts. Record any distribution-specific limitation rather than
expanding the bundled library set without review.

## Verification and acceptance

The implementation is ready when all of the following hold:

- `mise run package:linux` produces the expected AppImage and tarball on native
  x86_64 and aarch64 runners.
- The final ELF machine, AppImage runtime, public filename, and CI runner
  architecture agree.
- Required GLIBC imports do not exceed 2.35. Weak imports are reported and
  reviewed separately rather than silently treated as hard requirements.
- The dependency audit finds only the approved private xkbcommon closure and
  the explicit host-owned set. Each private library has matching licence data.
- `libfreetype.so.6` is absent from `DT_NEEDED`, and the FreeType License notice
  accompanies the statically compiled FreeType code.
- The six built-in theme notices, Ghostty notice, and terminal-graphics notice
  required by macOS packaging also appear in both Linux formats.
- The ELF and every private library have only approved relative runpaths. No
  file records a build path or resolves a private dependency outside the
  extracted bundle.
- The tarball extracts under one top-level directory and contains no
  AppImage-specific files.
- The extracted AppImage contains the same neutral payload bytes as the
  tarball plus only the declared AppImage files.
- Both artifacts launch under Xvfb and Mesa software Vulkan and pass the
  existing exact-byte Linux input smoke with Alacritty and Ghostty on both
  architectures.
- Desktop and AppStream metadata validators pass.
- Tool download tests reject altered bytes and package verification performs no
  network access.
- Each platform job records payload digests before upload. Assembly rejects a
  downloaded payload that differs from its platform digest manifest.
- Script tests prove wrong, missing, extra, empty, duplicate, or digest-mismatched
  release assets block publication.
- Manual non-publishing verification uploads the complete eight-file set
  without requiring or inspecting a tag or GitHub Release.
- Publishing uploads and remotely verifies the exact eight-file set before the
  final publish call. Every earlier failure leaves the release as a draft.
- `mise run verify` and the dependency licence audit pass on the final change.

For new package tests, observe at least one representative failure at the
intended assertion before accepting the passing result. Existing green smokes
remain regression evidence; they do not prove package relocation or dependency
isolation until run against the extracted public artifacts.

## Risks and fallback points

- GitHub-hosted ARM runners may have lower availability than x86_64 runners.
  Keep release jobs rerunnable and retain each verified platform artifact. Do
  not publish a release missing one of the declared architectures.
- appimagetool or its aarch64 build may fail on the selected host. Keep the
  neutral bundle and AppDir construction independent from image creation so a
  pinned replacement tool does not change the public payload.
- A private xkbcommon library may interact badly with a newer host X stack. The
  first fallback is to stop bundling the xkbcommon closure and document the
  required host package. Do not bundle XCB core or GPU libraries to mask the
  failure.
- Ubuntu 22.04 may become too old for a future Rust, Bun, Zig, or native
  dependency tool. Change the supported glibc floor deliberately and document
  it; do not let a hosted-runner image update change it accidentally.
- Xvfb with Mesa proves software-rendered startup and input, not every physical
  GPU driver. Perform one physical Vulkan launch per architecture when hardware
  is available and state any untested driver families.

## Alternatives considered

### Fully static musl executable

Rejected. Huterm must load the host Vulkan stack and vendor GPU modules. A musl
application does not remove those dynamic boundaries and can conflict with
glibc-built desktop and graphics libraries.

### Bundle the complete dynamic dependency closure

Rejected. Copying glibc, XCB core, Vulkan, Mesa, Fontconfig, or graphics drivers
can make the package less compatible with the host. AppImage's exclusion policy
keeps these libraries outside the image for the same reason.

### Statically link XCB and xkbcommon

Rejected for the first release. xkbcommon would require a separately pinned
native source build, while static XCB could coexist poorly with XCB loaded by
the graphics stack. Private shared xkbcommon libraries provide the useful
package reduction with less maintenance.

### Use cargo-packager for both Linux formats

Rejected. The current metadata cleanly describes the universal macOS app.
Linux needs a shared neutral payload, two native architectures, explicit
dependency ownership, and pinned AppImage runtime selection. Hiding those
rules behind another packager invocation would make release verification less
direct.

### Build aarch64 through QEMU on x86_64

Rejected as release evidence. It adds binfmt state, is slower, and does not
exercise a native ARM graphics and desktop stack. It remains useful as an
optional local compile check when a developer has configured emulation.

## References

- [AppImage concepts and dependency exclusions](https://docs.appimage.org/introduction/concepts.html)
- [AppImage motivation and traditional portable tarballs](https://docs.appimage.org/introduction/motivation.html)
- [appimagetool](https://github.com/AppImage/appimagetool)
- [AppImage type-2 runtime](https://github.com/AppImage/type2-runtime)
- [AppImage community library exclusion list](https://github.com/AppImageCommunity/pkg2appimage/blob/master/excludelist)
- [GitHub-hosted runner architectures](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)
- [Rust static and dynamic C runtime linkage](https://doc.rust-lang.org/reference/linkage.html#static-and-dynamic-c-runtimes)
- [libxkbcommon X11 support](https://xkbcommon.org/doc/current/group__x11.html)
- [libxkbcommon data lookup](https://xkbcommon.org/doc/current/custom-configuration.html)

## Claude review

Claude Fable 5.1 reviewed the first draft against the current repository on
2026-09-10. Its eleven findings were accepted after source checks. The review
added the FreeType link-mode and licence guard, complete built-in notices,
pre-upload platform digests, explicit rewrites for existing release tests, a
512-pixel hicolor icon, packaging job prerequisites, private-library package
provenance, the aarch64 Cargo audit target, required-check preservation,
architecture-specific cache keys, and explicit `SOURCE_DATE_EPOCH` and
`AppRun` rules.

The review could not reproduce the earlier Linux ELF inspection because its
read-only checkout did not contain that build. The implementation therefore
turns those observations into package-time checks and `/proc/<pid>/maps`
evidence rather than treating them as permanent facts.

## Unresolved questions

There are no decisions blocking implementation. The private xkbcommon set is a
candidate until the package script computes and validates the Ubuntu 22.04
closure on both architectures. Compatibility results may reduce that set, but
must not expand it into host-owned desktop or graphics libraries without a new
review.
