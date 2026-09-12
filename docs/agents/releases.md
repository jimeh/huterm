# Releases

Huterm uses Release Please to maintain one release pull request on `main`.
Merging that pull request creates a `v<version>` tag and a draft GitHub Release.
The same workflow passes Release Please's exact SHA, tag, and version outputs to
the reusable release workflow. There is no independent tag trigger.

The first public release is `v0.1.0`. Later versions follow conventional commit
types: `fix` produces a patch, `feat` produces a minor release, and a breaking
change produces a major release. Before 1.0, breaking changes increase the minor
version.

## Repository configuration

Install a GitHub App on the repository with read/write access to Contents,
Issues, and Pull requests. Add these GitHub Actions variables:

- `RELEASE_BOT_CLIENT_ID`: client ID of the installed GitHub App.
- `APPLE_TEAM_ID`: ten-character Apple Developer team ID.
- `APPLE_NOTARIZATION_KEY_ID`: ten-character App Store Connect API key ID.
- `APPLE_NOTARIZATION_ISSUER_ID`: App Store Connect API issuer UUID.

Add these GitHub Actions secrets:

- `RELEASE_BOT_PRIVATE_KEY`: PEM private key downloaded for the GitHub App.
- `MACOS_DEVELOPER_ID_APPLICATION_P12_BASE64`: base64-encoded Developer ID
  Application PKCS#12 file.
- `MACOS_DEVELOPER_ID_APPLICATION_P12_PASSWORD`: password for the PKCS#12 file.
- `APPLE_NOTARIZATION_KEY_P8_BASE64`: base64-encoded App Store Connect API `.p8`
  file.

The Developer ID certificate must belong to `APPLE_TEAM_ID`. The App Store
Connect key must match the configured key and issuer IDs and have permission to
submit notarization requests. Keep all four secret values out of the repository
and workflow logs.

Create a protected GitHub Environment named `release`. Restrict it to the
production branch and recovery tags, then add `SPARKLE_EDDSA_PRIVATE_KEY` as an
environment secret. Build and assembly jobs cannot read it. Only the publishing
job enters the environment.

## Sparkle signing authority

Prepare the pinned Sparkle tools and generate the production key on a trusted
Mac:

```sh
mise run sparkle:prepare
.native/sparkle/distribution/bin/generate_keys --account huterm
.native/sparkle/distribution/bin/generate_keys --account huterm -p \
  > assets/macos/SparklePublicKey
sparkle_key_dir="$(mktemp -d)"
.native/sparkle/distribution/bin/generate_keys --account huterm \
  -x "$sparkle_key_dir/huterm-sparkle-private-key"
```

Commit `assets/macos/SparklePublicKey`. The release package injects it as
`SUPublicEDKey`; the source `assets/macos/Info.plist` remains Sparkle-free. Store
the exact exported private-key contents as the protected
`SPARKLE_EDDSA_PRIVATE_KEY` environment secret and keep an encrypted offline
recovery copy. Ordinary builds and `mise run package:macos` need neither key.

```sh
gh secret set --repo jimeh/huterm --env release \
  SPARKLE_EDDSA_PRIVATE_KEY \
  < "$sparkle_key_dir/huterm-sparkle-private-key"
```

After checking the secret name with
`gh secret list --repo jimeh/huterm --env release`, delete the transient export.
GitHub never returns the secret value. Keep the Keychain item created by
`generate_keys` unless deliberately transferring update authority to another
trusted Mac.

Run `mise run smoke:macos-updater` on macOS to exercise framework loading,
controller startup, `canCheckForUpdates`, application-command dispatch, and the
unpackaged diagnostic. The smoke uses a fixture key and reserved-domain feed.
It never reads the production private key.

The public key is application identity; the private key grants update
authority. Rotate it only by shipping a replacement public key through an
update signed by the existing key. Losing the private key without a recovery
copy requires a separately distributed Developer ID signed package or manual
reinstallation.

## Release path

The release workflow separates validation, native platform builds, and final
assembly:

1. The preflight validates the requested SHA before checkout, revalidates the
   checked-out source, Cargo package versions, and committed schemas, then emits
   the SHA used by every build job. Publishing mode subsequently mints a
   short-lived token to validate the exact tag and draft target; manual
   verification does not receive that release credential.
2. An Apple Silicon runner runs `package:macos-release`, which builds the
   updater-enabled universal app, injects the committed public key and production
   feed, and copies the verified Sparkle framework. It signs every retained
   Sparkle and Huterm code object inside out, notarizes and staples the app, runs
   Gatekeeper, creates the SPDX SBOM and a fixture appcast, then uploads those
   payloads with their digest manifest.
3. Native Ubuntu 22.04 x86_64 and aarch64 runners independently build and verify
   the AppImage and tarball for their architecture. These jobs receive no Apple
   or GitHub release credentials and upload their two packages plus a digest
   manifest as intermediate Actions artifacts.
4. The assembly job downloads exactly those three platform artifacts, validates
   their inventories and digests, adds the committed schemas, writes
   `SHA256SUMS`, and uploads an immutable ten-file candidate.
5. The protected macOS publication job verifies the candidate artifact ID and
   digest, revalidates the draft, replaces the fixture appcast with one signed by
   the production EdDSA key, and rewrites `SHA256SUMS`. It uploads the exact
   assets, verifies GitHub's sizes and SHA-256 digests, attests the macOS ZIP and
   SPDX SBOM, and publishes only after every check passes.

Each platform job publishes its immutable artifact under its own actual
attempt-qualified name and exposes that name as a job output. Assembly downloads
the names from its direct `needs` jobs, so a partial rerun can combine a retained
successful producer from an earlier attempt with a newly successful producer.
Producer artifacts use the same 30-day retention window as GitHub workflow
reruns.
This follows GitHub's contracts that [failed-job reruns include dependent jobs]
and [job outputs are available to downstream jobs].

[failed-job reruns include dependent jobs]: https://docs.github.com/en/rest/actions/workflow-runs#re-run-failed-jobs-from-a-workflow-run
[job outputs are available to downstream jobs]: https://docs.github.com/en/actions/how-tos/write-workflows/choose-what-workflows-do/pass-job-outputs

The public asset set is:

```text
Huterm-<version>-macOS-universal.zip
Huterm-<version>-macOS-universal.spdx.json
appcast.xml
Huterm-<version>-Linux-x86_64.AppImage
Huterm-<version>-Linux-x86_64.tar.gz
Huterm-<version>-Linux-aarch64.AppImage
Huterm-<version>-Linux-aarch64.tar.gz
huterm.schema.json
huterm-theme.schema.json
SHA256SUMS
```

Any failure before the final publish call leaves the GitHub Release as a draft.
The signing helper restores the runner's keychain configuration and deletes its
temporary keychain in a `finally` path. The workflow also runs cleanup with
`always()` in case the build step ends early.

The public ZIP is created after stapling. Re-signing the app after notarization
would invalidate the ticket, so the post-staple path only verifies signatures.
The local `mise run package:macos` task remains an unsigned, Sparkle-free
package check. `mise run package:macos-release` is the unsigned updater-enabled
package check. It requires the committed public key, never the private key.

## Manual verification

Run the `Release` workflow manually with `publish` unchecked to exercise the
complete package path. Select the branch to run from and enter its
exact 40-character HEAD SHA, or a SHA from `main`, plus the matching Cargo
version; leave the tag empty. The workflow validates the
source, builds both native Linux architectures, builds and signs both macOS
slices, notarizes and staples the app, validates the SPDX SBOM and a non-public
fixture appcast, and uploads the same ten-file inventory as an Actions artifact
retained for seven days.

Before checkout, the workflow requires the SHA to be on `main` or to match the
exact branch commit selected by a manual, non-publishing dispatch. It verifies
the checkout and Cargo versions again before exposing the signing and
notarization credentials. Publishing always requires ancestry on `main`.

Verification mode does not inspect, create, update, or publish a GitHub Release
or tag. It never receives the production Sparkle private key or attestation
authority. It does submit the app to Apple's notarization service and creates
the temporary Actions artifact.

## Manual recovery

To recover an existing draft release, run the same workflow with `publish`
checked. Enter the exact 40-character SHA, `v`-prefixed tag, and version from
that draft. The workflow revalidates the release and rebuilds the artifacts. It
replaces the ten expected assets when they already exist, but refuses to
publish if the draft contains any unexpected asset.

## Linux package contract

Both Linux formats contain the same neutral payload: `bin/huterm`, the desktop
entry, AppStream metadata, the 512-pixel icon, package provenance, and third-party
notices. The tarball does not contain `AppRun`, `.DirIcon`, an AppImage runtime,
its redistribution notices, or other AppImage-only files. The AppImage adds a
declared launch envelope around that byte-identical payload. The envelope holds
only three launcher/icon symlinks, a root copy of the desktop entry with
appimagetool's exact `X-AppImage-Version` addition, and exact-tag notices for the
type-2 runtime, musl, libfuse, squashfuse, zstd, zlib, and mimalloc. Package
verification rejects any other envelope path or changed bytes.

Linux releases require x86_64 or aarch64, glibc 2.35 or newer, X11 or XWayland,
and a working Vulkan driver. The package privately carries only the xkbcommon
library family. glibc, the ELF loader, X11/XCB, Vulkan and GPU drivers remain
host-owned. FreeType and Ghostty VT are statically linked; verification rejects
an unexpected dynamic FreeType dependency.

Before the first public Linux release, launch each native package on Ubuntu
22.04 and Ubuntu 24.04. Exercise the AppImage through FUSE when available and
with `--appimage-extract-and-run` when FUSE is unavailable. Also run the
tarball's `bin/huterm` before and after moving the extracted directory, and
verify desktop integration by installing the included desktop entry, AppStream
metadata, and icon under the corresponding `$XDG_DATA_HOME` paths. Record any
physical-GPU or native-Wayland evidence separately; CI covers X11 under Xvfb
with Mesa software Vulkan.

## Privacy and hardware checks

The package includes privacy descriptions for protected macOS resources that
Huterm or a child process may request. The release signature carries the seven
terminal-host entitlements for Apple Events, microphone, camera, contacts,
calendars, location, and Photos. It does not allow JIT, library-validation
bypass, DYLD environment variables, application groups, or keychain groups.

Automated packaging proves both executable slices, signing structure,
notarization, stapling, and Gatekeeper acceptance. It does not prove the native
Intel UI on Intel hardware or each child-process consent prompt. Test those
paths manually before treating them as covered.
