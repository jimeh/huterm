# macOS self-updates

Status: proposed implementation plan, following investigation and independent
Claude reviews on 2026-09-10. Review findings, the later macOS-only scope
decision, and the update-check configuration contract are incorporated below.

Add macOS self-updates without replacing Huterm's current release pipeline or
building a custom update interface. Use Sparkle's standard macOS interface and
updater engine. Publish an SPDX software bill of materials (SBOM) and GitHub
build-provenance and SBOM attestations for the installable ZIP at the same time.

## Outcome and settled direction

The first macOS release with this feature includes a signed Sparkle framework,
a working **Check for Updates...** application-menu item, and an appcast published
with the existing GitHub Release. Huterm config may explicitly enable or disable
scheduled checks and set their interval. When those settings are absent, Huterm
does not override Sparkle: on a fresh profile Sparkle asks for permission on the
second launch and uses its standard daily interval. Downloading and installing
updates remains opt-in through Sparkle. A separate in-app Huterm settings surface
is outside this delivery.

Expose one dependency-neutral `check_for_updates` application command. Its
macOS implementation stays in `huterm-gpui`, since update discovery,
presentation, installation, and relaunch belong to the packaged desktop client.
Other platforms return an unavailable-command diagnostic until their packaging
and updater work lands. `huterm-core` and terminal snapshots do not gain update
state.

Do not add a public updater trait or shared update state machine in this
delivery. A private macOS adapter only needs to initialize Sparkle, start a
user-requested check, and shut down retained integration state. If Huterm later
replaces the standard interface, introduce a shared state model from the
concrete UI events required at that point.

AppImage packaging, Linux update UI, and command-palette presentation are
explicit non-goals. They are moving in parallel and will define the artifact and
discovery contracts for a later Linux updater plan.

## User contract

- Put **Check for Updates...** in the Huterm application menu after **About
  Huterm** and before settings and services.
- A manual check always brings Sparkle's current update session to the front,
  including when scheduled checks are disabled.
- Add these optional settings to Huterm's external config:

  ```toml
  [updates]
  automatic_checks = true
  check_interval_hours = 24
  ```

  `automatic_checks = true` enables scheduled checks without Sparkle's consent
  prompt; `false` disables them without prompting. When it is absent, Huterm
  leaves the choice to Sparkle, which asks on the second launch of a fresh
  profile and persists the answer in its normal preferences.
- `check_interval_hours` accepts whole hours with a minimum of 1. When absent,
  Huterm leaves Sparkle's stored interval untouched; on a fresh profile that is
  Sparkle's 24-hour default. An explicit interval does not itself grant consent
  to scheduled checks.
- The generated default config documents both settings but leaves them
  commented out, preserving Sparkle's default consent flow.
- Leave `SUAutomaticallyUpdate` disabled by default. An update requires a user
  choice before download or installation unless the user opts into Sparkle's
  automatic-update setting.
- Sparkle may ask macOS for administrator authorization when the installed app
  is not writable by the current user.
- An install-triggered relaunch uses normal AppKit application termination. It
  must pass through Huterm's existing running-job assessment. Canceling that
  assessment keeps the current app running, but the downloaded update remains
  armed. Running **Check for Updates...** again reopens the update and lets the
  user retry termination. A later ordinary Quit may also install and relaunch
  the armed update, which the confirmation copy and native tests must reflect.
- Development binaries and incomplete app bundles do not check the production
  feed. The manual command reports that updates are available only in a
  packaged Huterm application.

## Current seams and constraints

`crates/huterm-protocol/src/command.rs` owns the command catalog and already
contains application-scoped desktop commands. `crates/huterm-gpui/src/desktop.rs`
builds the macOS application menu, while
`crates/huterm-gpui/src/desktop/windows.rs` dispatches application commands and
owns desktop startup and quit coordination. Add the new command through those
paths instead of installing an independent menu callback.

`crates/huterm-config/src/lib.rs` owns the dependency-neutral raw config and
schema, while `crates/huterm-gpui/src/config.rs` validates settings consumed by
the desktop. Add an optional top-level `updates` section there, with
`Option<bool>` for `automatic_checks` and `Option<u32>` for
`check_interval_hours`. Keep the unit in the key, matching existing duration
settings. The section is accepted on every platform, but only the packaged
macOS client consumes it until Linux updater support lands.

The native AppKit bridge in `crates/huterm-gpui/src/native_quit.rs` already
confines Objective-C operations that cannot be expressed through GPUI. Keep the
Sparkle controller in a separate `native_updater.rs` module with a safe Rust
interface. It may reuse the existing target-specific `objc` dependency, but it
must not add Sparkle, AppKit, or Objective-C types to protocol or core APIs.

The root package uses `cargo-packager` to create a universal `Huterm.app`.
`scripts/release-macos.ts` then signs every Mach-O, signs the outer application,
notarizes a temporary ZIP, staples the application, verifies it, and creates the
public ZIP. Sparkle adds a versioned framework, helper executables, and nested
bundle containers. Signing only their Mach-O files is insufficient. The release
pipeline must sign Sparkle's retained nested code and bundle containers from the
inside out before signing `Sparkle.framework` and `Huterm.app`.

`cargo-packager` 0.11.8 currently generates `CFBundleVersion` from the UTC build
time, while Sparkle uses that value for update ordering. Two builds of the same
release can therefore acquire different update identities. Override
`CFBundleVersion` in `assets/macos/Info.plist` with the stable three-part package
version, mark it for Release Please updates, add the plist to Release Please's
generic extra files, and assert that both bundle version fields match the
validated release version. This must ship in the first Sparkle-enabled build.

GitHub Release asset validation currently requires an exact name, size, and
SHA-256 match before publication. Keep that invariant when adding the appcast.
Update `assetNames`, `verifyLocalAssets`, the manual Actions-artifact path list,
and the remote inventory checks together so no producer or consumer has a
different release shape.
The draft release must remain unpublished if framework verification, archive
signing, appcast validation, or asset verification fails.

The release workflow does not currently generate an SBOM or call GitHub's
artifact-attestation action. Its release job has only `contents: read`, and the
v0.4.0 release contains the application ZIP, two schemas, and `SHA256SUMS`.
`gh attestation verify` returns no provenance attestation for that ZIP. Treat
SBOM generation and both attestations as part of this delivery, not as an
optional release hardening follow-up.

## macOS design

### Pin and prepare Sparkle

Use the official prebuilt Sparkle 2 distribution. The implementation must
recheck the latest release and apply Huterm's three-day release-age policy; this
plan was researched against Sparkle 2.9.6.

Record and enforce the selected framework's minimum macOS version. Set Huterm's
`minimumSystemVersion` to at least the stricter of GPUI's and Sparkle's runtime
requirements, and verify the resulting `LSMinimumSystemVersion`. Do not accept a
new Sparkle release that silently drops a macOS version Huterm still claims to
support.

Add a small manifest containing the selected version, official release URL,
archive digest, expected framework identity, and license provenance. A
`sparkle:prepare` Mise task downloads into ignored `.native/sparkle`, buffers the
response before writing, verifies the digest before extraction, and verifies the
expected framework and tools after extraction. A check mode validates existing
content without repairing mismatches. CI and release builds use only that
verified directory.

Do not commit the binary framework. Commit the manifest, preparation code,
tests, license notice, and `.gitignore` entry. Add the new non-Cargo dependency
to repository license and source-policy checks rather than assuming
`cargo deny` covers it.

Embed `Sparkle.framework` at
`Huterm.app/Contents/Frameworks/Sparkle.framework` and link the macOS executable
through cargo-packager's macOS framework configuration, which preserves the
versioned symlinks. Since Huterm is not sandboxed, remove the unused Sparkle XPC
services from the copied application bundle before signing. Do not alter the
verified `.native/sparkle` source tree. Retain the normal updater application
and autoupdate helper.

Weak-link the verified framework and resolve its Objective-C classes through the
runtime. This lets unpackaged builds report that updates are unavailable when
Sparkle is absent without adding a custom `dlopen` path. Add a root-package
`build.rs` for the final `huterm` executable's framework search path,
`-weak_framework Sparkle`, and
`@executable_path/../Frameworks` runtime search path. Do not emit the rpath from
`huterm-gpui`, since Cargo does not propagate a dependency build script's raw
link arguments into the root executable. Development and test launches point
`DYLD_FRAMEWORK_PATH` at the verified `.native/sparkle` framework when they need
to exercise the controller.

Add `sparkle:prepare` to every Mise task that compiles or launches `huterm-gpui`
on macOS, including build, Clippy, typecheck, tests, smokes, and packaging. The
corresponding macOS CI jobs must materialize Sparkle before the first Cargo
invocation. Linux tasks remain independent of it.

Package checks must prove:

- the framework exists at the standard bundle location and its symlinks remain
  symlinks;
- the main executable resolves Sparkle through its packaged framework path and
  does not depend on a build-machine path;
- the framework and retained executables contain both `arm64` and `x86_64`;
- neither `Versions/B/XPCServices` nor its top-level framework symlink remains
  in the final bundle; and
- Sparkle's license notice is present in packaged resources.

### Initialize and invoke Sparkle

Apply the production-bundle gate before constructing Sparkle objects. Create one
`SPUStandardUpdaterController` with `startingUpdater:NO` after GPUI has
initialized the packaged application, retain it for the application lifetime,
then call `startUpdater:` on its `SPUUpdater` and capture the `NSError` result.
Before returning control to the next main-run-loop iteration, apply only explicit
Huterm update settings: map `automatic_checks` to
`automaticallyChecksForUpdates` and convert `check_interval_hours` to seconds for
`updateCheckInterval`. This lets the configured values take effect before
Sparkle's first scheduled cycle while still surfacing startup errors. Expose a
safe operation that checks `canCheckForUpdates` and asks the updater to start or
focus a manual check. All Objective-C work stays on AppKit's main thread.
Initialization failure is nonfatal to terminal use, but the menu command reports
the stored error and the packaged smoke fails.

Sparkle persists both runtime properties in the application's user defaults and
automatically reschedules after either changes. Apply an explicit
`automatic_checks` value on every launch because Sparkle distinguishes a missing
preference from an explicit `false` when deciding whether to show its consent
prompt. Apply an explicit interval only when it differs from Sparkle's current
value, and apply changes after config reload on the main thread. When a field is
absent, do not call its setter. If an explicit field is later removed, return
ownership to Sparkle without deleting the current persisted value; this avoids
erasing an answer recorded by Sparkle's own consent prompt. Consequently,
"absent" means "no Huterm override," not "reset Sparkle preferences." Document
that distinction next to the generated config example.

Add `check_for_updates` to the protocol catalog as an application-scoped command
with no arguments. Route it through the existing `InvokeApp` action and
`Desktop::invoke` path. Do not give it a default terminal keybinding. The native
menu is the discoverable macOS entry point, and an unbound command cannot steal
terminal input.

Add these Info.plist values:

- `CFBundleVersion` equals the three-part Cargo package version and changes only
  when Release Please changes that version;
- `SUFeedURL` points at
  `https://github.com/jimeh/huterm/releases/latest/download/appcast.xml`;
- `SUPublicEDKey` contains the public half of Huterm's Sparkle EdDSA key;
- `SUVerifyUpdateBeforeExtraction` is true; and
- `SURequireSignedFeed` is true.

Do not put `SUEnableAutomaticChecks` or `SUScheduledCheckInterval` in the plist.
Their absence is required for the optional Huterm config and Sparkle's
second-launch consent behavior to coexist. Continue to leave
`SUAutomaticallyUpdate` unset so its default remains false.

Do not add new application entitlements. If Sparkle proves it needs one in the
packaged application, stop and review that requirement against the existing
seven-entitlement allowlist instead of widening it during implementation.

### Sign and verify nested code

Replace the current flat Mach-O signing loop with an explicit inside-out plan
for nested code. Discovering a Mach-O remains useful for completeness checks,
but bundle signing order must be modeled separately. Sign retained Sparkle
helpers with their required metadata, then the updater application, the
framework, the Huterm executable with Huterm's approved entitlements, and the
outer application. Do not use `codesign --deep` to create signatures.

Verification must enumerate every nested code object and assert its Developer
ID team, Hardened Runtime, secure timestamp, and expected entitlements. Sparkle
helpers must not inherit Huterm's terminal-host entitlements. Run strict
`codesign` validation, notarization, staple validation, and Gatekeeper assessment
before creating the public ZIP, as the current pipeline does.

## Appcast and release design

Generate `appcast.xml` only after the final stapled ZIP exists. Use Sparkle's
official signing tools from the same pinned distribution to sign the archive
and the feed. Read the exact version, tag, SHA, file size, and download URL from
the already validated release inputs. Do not infer release identity from the
working branch or a mutable latest URL.

The enclosure URL uses the immutable tagged asset:

```text
https://github.com/jimeh/huterm/releases/download/v<version>/Huterm-<version>-macOS-universal.zip
```

The application feed uses GitHub's stable latest-release redirect. This matches
the repository's existing schema URLs and keeps publication atomic: upload the
ZIP and appcast to the validated draft, verify all draft assets, then make that
release public and latest. A client cannot see the new feed until the release
and its referenced archive are public together.

The initial appcast may contain only the current release. Do not add delta
updates yet. The current ZIP is small, and deltas would require retaining and
validating earlier archives during every release. Add bounded history before
raising the minimum macOS version or introducing release channels, since those
features may require Sparkle to select an older compatible item.

Use the immutable GitHub release page as the release-notes link for the first
delivery. A separate signed release-notes asset can follow if the standard
Sparkle view does not present the GitHub page well enough in native QA.

Treat the EdDSA private key as release authority:

- generate it outside CI with Sparkle's tool;
- store it as a secret in a protected `release` GitHub Environment and keep an
  offline recovery copy;
- pass it to `sign_update` or `generate_appcast` only through standard input via
  `--ed-key-file -`, never as an argument or environment inherited by unrelated
  processes;
- expose the production secret only to publishing runs, never to manually
  dispatched branch verification;
- never write it into the checkout, release artifact, cache, or logs; and
- document rotation and lost-key recovery before the first public feed is
  enabled. Losing this key ends in-app updates for existing ZIP installations
  unless Huterm adds Sparkle's Developer ID signed DMG recovery path. The
  verified offline copy is therefore a hard launch gate, not a suggestion.

Set `environment: release` only on the publishing job. Publishing uses the
production key there, after the signed ZIP has crossed the verified Actions
artifact boundary. Manual non-publishing verification creates a uniquely named
throwaway key in the temporary release keychain, exports it for the one local
signing operation, and deletes the exported file and keychain during the
existing cleanup. Never expose production update authority to code selected by
a manual branch dispatch.

Run Sparkle's appcast generator in a temporary directory containing only the
final ZIP. It reuses nearby appcasts and release-note files, so running it in
`dist` would make output depend on stale files and break the exact-directory
inventory. Copy the completed `appcast.xml` into `dist` and never mutate it
after signing.

Extend local and remote asset validation to include `appcast.xml`. Before
publication, parse the uploaded draft feed and prove its version, tag URL,
archive size, and EdDSA signature match the exact local ZIP. Verify that
signature with the public key read from the stapled app's `SUPublicEDKey`, not
merely with the private key that produced it. Pin the expected public key in a
committed release constant and require the plist value to match it. Manual runs
perform the same check with their throwaway key pair. After publication, poll
the unauthenticated `latest/download/appcast.xml` URL and the enclosure URL, then
compare their bytes and digests with the verified local assets.

A public probe failure is a release incident. Fail the workflow and follow the
release runbook: set the prior good release as latest with `make_latest=true`,
or mark the bad release as a prerelease if no prior release can be restored.
Keep that recovery manual. GitHub has no separate "unset latest" operation, and
automatic rollback after publication would widen this delivery's authority and
failure modes.

Manual non-publishing release verification generates and validates an appcast
locally using the supplied version and a non-public fixture URL. It uploads the
appcast only as an Actions artifact. It does not inspect or mutate a GitHub
Release and does not make the feed visible.

The old-to-new native test uses a disposable staging bundle whose plist contains
a staging feed URL and the matching throwaway public key. It never mutates the
production plist, signs with the production EdDSA key, or uploads the staging
feed to a public Huterm Release.

## Release SBOM and GitHub attestations

Generate an SPDX 2.3 SBOM named
`Huterm-<version>-macOS-universal.spdx.json` from the final stapled application.
Make release builds use a pinned `cargo-auditable` wrapper so each architecture
slice carries its resolved Rust dependency tree, then scan the packaged app with
a pinned Syft release. Select Syft's Rust audit-binary cataloger explicitly
rather than relying on its changing defaults. Syft can then recover
cargo-auditable metadata and describe the frameworks and resources it can
identify in the bundle.

Add a small deterministic augmentation step for shipped components that binary
inspection cannot recover, especially the statically linked Ghostty native
source, its `uucode` and `highway` Zig packages, Sparkle, and the patched local
crates under `[patch.crates-io]`. Read their names, versions, digests, licenses,
source URLs, and vendor provenance from Huterm's checked manifests and
provenance files. Do not include Bun, Zig, cargo-packager, or other build-only
tools in the runtime SBOM.

Build the Rust package list as the union of the two architecture slices and
annotate architecture-specific entries. Requiring identical lists would reject
valid target-specific dependencies, while scanning only the universal binary
would hide which slice supplied them. Reject the result unless the pinned
`pyspdxtools` validator accepts it as SPDX 2.3 and it contains Huterm,
representative resolved Rust crates, Sparkle, Ghostty, `uucode`, `highway`, and
the expected patched-crate provenance.

Publish the SBOM as a normal release asset and include its digest in
`SHA256SUMS`. It describes the installable ZIP, but it is not a signature and
does not replace Sparkle's EdDSA archive signature, Apple's code signature, or
GitHub's build provenance.

Use the current `actions/attest` action rather than the deprecated
`attest-build-provenance` and `attest-sbom` wrappers. Pin its full commit SHA and
apply Huterm's three-day action release policy. Create two attestations whose
subject is the exact macOS ZIP:

- the default SLSA build-provenance attestation; and
- an SBOM attestation whose predicate is the published SPDX document.

Keep attestation authority out of manual branch verification. Split the current
release job at the signed-artifact boundary:

1. The macOS build job retains `contents: read`, builds, signs, notarizes,
   staples, and generates and validates the SBOM. It uploads the verified ZIP,
   schemas, and SBOM as an Actions artifact for every run. Pass the upload
   action's exact artifact ID and digest as job outputs. Add pinned
   `cargo-auditable`, Syft, and `pyspdxtools` to its explicit Mise install list.
   A non-publishing run also generates a fixture appcast with its throwaway key,
   builds `SHA256SUMS`, and includes both in its verification artifact.
2. A publishing-only macOS job downloads the candidate by exact artifact ID,
   verifies the artifact digest, checks out `inputs.sha` with tags, installs the
   explicitly named Rust, Bun, and `pyspdxtools` tools, materializes the pinned
   Sparkle signing tools, and revalidates the transferred files plus the exact
   draft. It signs the appcast with the production key in its temporary isolated
   directory, copies it into `dist`, generates the final `SHA256SUMS`, then runs
   the complete local asset validation. Pass the draft release ID from the build
   job, re-resolve it, and require both values to match before mutation.
3. Grant only the publishing job `contents: read`, `id-token: write`,
   `attestations: write`, and `artifact-metadata: write`, and attach the protected
   `release` Environment. Add the same three non-content permissions to the
   Release Please caller job because a reusable workflow cannot exceed its
   caller's permission grant. Continue to mint a separate short-lived GitHub App
   token for draft upload and publication. `actions/attest` uses the job token;
   release mutation uses only the App token.
4. Before attesting, require `GITHUB_SHA == inputs.sha` and allow only a normal
   Release Please run on `refs/heads/main` or an explicitly documented recovery
   dispatch on `refs/tags/<tag>`. Checking out `inputs.sha` is insufficient,
   because GitHub records the workflow run's SHA and ref in the attestation.
5. Upload and verify every draft asset, create both attestations through
   `sbom-path` and the default provenance mode, verify them against the local
   ZIP, then publish the draft. Any earlier failure leaves the release in draft.
   A retry may create a duplicate attestation for identical bytes, which is
   harmless. An attestation for bytes that were never published does not
   authorize a different digest.

Verification must constrain more than the repository owner. Check the expected
repository, signer workflow, source SHA, and source ref. Verify provenance with
`gh attestation verify`, then verify the SBOM claim with the SPDX predicate type
`https://spdx.dev/Document/v2.3`. Add both commands to the release guide so users
can verify a downloaded ZIP. Document the normal `refs/heads/main` source ref and
the exact tag ref used for a recovery publication. The post-publication probe
repeats both checks against the downloaded release asset. Compare the parsed
SPDX JSON in the attestation with the parsed release asset, since GitHub embeds
the document as a predicate rather than preserving the original file bytes.

## Deferred Linux follow-up

AppImage packaging and command-palette work are moving in parallel. This plan
does not choose Linux artifact names, updater metadata, UI placement, or a
self-replacement implementation. Once those contracts merge, write a separate
two-version AppImage updater plan. Reuse the dependency-neutral
`check_for_updates` command and extend SBOM and provenance attestations to the
AppImage and its update metadata in that delivery.

## Implementation sequence

1. Replace the timestamp-derived `CFBundleVersion` with the Release
   Please-managed package version, declare the minimum macOS version, and extend
   metadata checks before introducing an updater that depends on either value.
2. Add Sparkle provenance, preparation, license, and deterministic source checks.
   Make macOS development and packaging commands materialize the pinned
   framework before compilation.
3. Embed and link the framework in the universal application. Extend package
   verification before adding runtime behavior, remove unused XPC services only
   from the bundle copy, and put the distributable rpath in the root package.
4. Add the optional `[updates]` config section, validation, schema descriptions,
   and commented default-config examples. Add the `check_for_updates` command,
   native controller, config-reload handling, standard menu item,
   production-bundle gate, and error reporting. Extend config, command, and real
   AppKit menu tests as each path is added.
5. Add EdDSA public configuration and private-key release inputs. Rework nested
   signing and prove the signed framework survives notarization, stapling, and
   final ZIP creation.
6. Generate, sign, validate, upload, and publicly probe the appcast within the
   existing draft-release transaction. Isolate generator inputs and verify the
   signature against the public key embedded in the stapled app. Update the
   release guide with key setup, manual verification, normal publication, and
   incident recovery.
7. Make release builds auditable, generate and validate the SPDX SBOM from the
   stapled app with explicit Rust cataloging and manifest-backed native
   augmentation, and add it to the exact release inventory and `SHA256SUMS`.
8. Split build from publication at the verified Actions-artifact boundary. Add
   exact artifact handoff, checkout and source-ref guards, caller permission
   grants, and publishing-only GitHub provenance and SBOM attestations. Verify
   both claims and keep the draft unpublished on any failure.
9. Regenerate the config schema and add the update settings and new command to
   the README tables. Update `AGENTS.md` so its unsafe Objective-C confinement
   rule names both native bridge modules. Document SBOM download and both
   attestation-verification commands in the release guide.
10. Exercise an actual old-to-new packaged update before enabling the production
    feed. Include canceled and retried Huterm job confirmation during relaunch.

## Verification

### Automated checks

- Unit-test source manifests, archive digest rejection, safe extraction,
  expected framework layout, appcast generation, XML escaping, exact release
  identity, URL construction, asset inventory, and public-probe validation.
- Extend command-catalog tests for application scope and unsupported-platform
  behavior. Test updater initialization failure and repeated manual checks
  without calling the network.
- Test config absence, explicit true and false values, interval conversion, the
  one-hour lower bound, and reload transitions. Prove an absent field never calls
  its Sparkle setter, an explicit interval does not enable scheduled checks, and
  the manual command remains available when `automatic_checks` is false. Confirm
  the generated default document leaves both fields commented out.
- Extend `smoke:macos-menus` to inspect the real **Check for Updates...** menu
  item and prove it dispatches the production action after startup and keymap
  reload.
- Add a packaged macOS updater smoke with fixture plist values and no production
  network access. Prove the framework loads from the bundle, the controller is
  created, `startUpdater:` succeeds, `canCheckForUpdates` is true, an unpackaged
  binary returns the expected diagnostic, and the menu dispatches the production
  command. Do not claim that this smoke drives Sparkle's modal update interface.
- Extend package and release-script tests for the nested signing order, exact
  framework inventory, helper entitlements, appcast asset, manual
  non-publishing behavior, isolated appcast generation, embedded-public-key
  verification, and failure-before-publication guards.
- Test that release builds embed auditable dependency data in both Mach-O
  slices. Validate their architecture-aware union, the SPDX document, its
  runtime-only component set, exact release identity, deterministic augmentation,
  size limit, asset digest, and rejection of missing Rust, patched-crate,
  Sparkle, Ghostty, `uucode`, or `highway` evidence.
- Exercise the split workflow with fixtures that prove manual verification has
  no production EdDSA key or attestation permission, the Release Please caller
  grants every requested permission, publication receives the exact artifact ID
  and digest, both jobs install their named tools, `GITHUB_SHA` and ref match the
  release, the SBOM is part of the exact draft inventory, and attestations run
  after draft asset verification but before publication.
- Run `mise run check:scripts`, focused Rust tests, `mise run package:macos`,
  the updater and menu smokes, `mise run smoke:macos-quit`, and finally
  `mise run verify`.

### Native release evidence

Automated fixtures cannot prove authorization, Gatekeeper, or GitHub's public
redirect behavior. Before release, install the older notarized build in
`/Applications`, serve a newer signed candidate through the staging feed, and
verify:

- Sparkle shows its standard interface and release link;
- an untouched fresh profile receives Sparkle's automatic-check consent prompt
  on its second launch, while explicit true and false config values suppress the
  prompt and produce the requested scheduled-check state;
- an explicit interval is reflected in Sparkle's updater state, and removing an
  explicit setting stops Huterm from overriding the persisted state on later
  reloads;
- a standard-user installation receives the expected authorization prompt;
- canceling Huterm's live-job confirmation leaves the old app and PTYs usable;
- rerunning **Check for Updates...** after cancellation retries the armed update;
- an ordinary later Quit also installs and relaunches an armed update;
- approving either retry closes the approved jobs, installs the new app, and
  relaunches it;
- the relaunched app reports the new version and preserves external config and
  restore data;
- `codesign`, `spctl`, stapler validation, and an unauthenticated download all
  succeed against the exact candidate; and
- the downloaded ZIP passes both provenance and SPDX attestation verification,
  and the published SBOM matches the attested predicate and `SHA256SUMS`.

Record the old and new versions, SHAs, feed digest, ZIP digest, SBOM digest,
attestation IDs, macOS version, hardware architecture, install location, and
result. Cross-compilation and a temporary writable bundle do not replace this
check.

## Accepted tradeoffs and rejected alternatives

Standard Sparkle UI gives macOS users familiar behavior and keeps signing,
authorization, installation, and relaunch policy in a mature platform tool. Its
cost is a macOS-only binary framework and more demanding nested signing.

Velopack is not selected. It would still require Huterm to build the in-app
update UI and would replace the current packaging and release contract to gain a
shared backend. That migration does not pay for itself while Sparkle's native UI
is a product goal.

Embedding cargo-auditable metadata adds a few kilobytes to each architecture
slice and makes resolved crate versions inspectable in shipped binaries. That is
an acceptable cost for an SBOM derived from the actual application rather than
from the build checkout. The explicit Sparkle and Ghostty augmentation remains
necessary because a binary scanner cannot reliably infer every framework and
statically linked native source component.

A custom `SPUUserDriver` is not selected for the first macOS delivery. Sparkle
supports it without changing appcast or archive formats, so Huterm can adopt a
custom GPUI interface later while retaining Sparkle's engine. Building that UI
now would add permission, release-note, download, extraction, error, install,
and relaunch states without improving the chosen first experience.

Direct ZIP replacement code is rejected. It would make Huterm own signature
policy, partial downloads, process coordination, rollback, privilege escalation,
filesystem replacement, and relaunch. Those are the hard parts of an updater,
not useful product differentiation.

## Remaining questions

- Does GitHub's unauthenticated `latest/download/appcast.xml` redirect meet the
  feed's cache and availability needs under real release traffic? Keep it as the
  default. Move to a dedicated static feed only if the public integration probe
  demonstrates a concrete failure.

## Primary references

- [Sparkle installation and distribution](https://sparkle-project.org/documentation/)
- [Sparkle publishing and appcast signing](https://sparkle-project.org/documentation/publishing/)
- [Sparkle customization and security settings](https://sparkle-project.org/documentation/customization/)
- [Sparkle updater API reference](https://sparkle-project.org/documentation/api-reference/Classes/SPUUpdater.html)
- [Sparkle custom user interfaces](https://sparkle-project.org/documentation/custom-user-interfaces/)
- [Sparkle manual signing for non-Xcode release workflows](https://sparkle-project.org/documentation/sandboxing/#code-signing)
- [Velopack Rust integration](https://docs.velopack.io/getting-started/rust)
- [GitHub artifact attestations](https://docs.github.com/en/actions/concepts/security/artifact-attestations)
- [GitHub `actions/attest`](https://github.com/actions/attest)
- [GitHub attestation verification](https://docs.github.com/en/actions/how-tos/secure-your-work/use-artifact-attestations/use-artifact-attestations#verifying-an-attestation-for-sboms)
- [cargo-auditable](https://github.com/rust-secure-code/cargo-auditable)
- [Syft](https://github.com/anchore/syft)
- [SPDX tools-python validator](https://github.com/spdx/tools-python)
