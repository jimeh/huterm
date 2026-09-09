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

## Release path

The release workflow does the following on an Apple Silicon macOS runner:

1. Checks out the exact Release Please SHA with full tag history.
2. Confirms that the checkout, tag, draft release target, and Cargo package
   versions match that SHA and version, and that `origin/main` contains the SHA.
3. Builds the existing universal `Huterm.app` with arm64 and x86_64 slices.
4. Imports the Developer ID identity into a temporary keychain.
5. Signs each Mach-O and the app with a secure timestamp and Hardened Runtime,
   then checks the authority, team, runtime flag, timestamp, and entitlements.
6. Submits a temporary ZIP to Apple's notary service, staples the accepted
   ticket to the app, validates the ticket, and runs Gatekeeper assessment.
7. Creates `Huterm-<version>-macOS-universal.zip` from the stapled app and
   writes `SHA256SUMS`.
8. Uploads both files to the draft, checks the exact remote names, sizes, and
   SHA-256 digests, then publishes the release.

Any failure before the final publish call leaves the GitHub Release as a draft.
The signing helper restores the runner's keychain configuration and deletes its
temporary keychain in a `finally` path. The workflow also runs cleanup with
`always()` in case the build step ends early.

The public ZIP is created after stapling. Re-signing the app after notarization
would invalidate the ticket, so the post-staple path only verifies signatures.
The local `mise run package:macos` task remains an unsigned package check.

## Manual recovery

Use the `Release` workflow's manual dispatch only for an existing draft release.
Enter the exact 40-character SHA, `v`-prefixed tag, and version from that draft.
The workflow revalidates the release and rebuilds the artifacts. It replaces the
two expected assets when they already exist, but refuses to publish if the draft
contains any unexpected asset.

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
