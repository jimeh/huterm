# Vendored dependencies

## Reproducing local changes

[sources.json](sources.json) pins each published crate archive by URL and SHA-256,
records its upstream VCS metadata, and lists its patches in application order.
Each patch has a stable name, a description, and an upstream link when available.
Keep each coherent fix together. GPUI has separate file-drop and explicit-float
patches, plus hidden-window creation, X11 native-handle, application-lifetime,
and macOS offscreen-display and per-window frame-constraint fixes. The sys crate
has separate CPU, build-script watch-path, license, Zig 0.16 migration, and
build-source staging patches.

Normal Cargo builds use the fully patched vendored source through
`[patch.crates-io]`. They do not apply patches. Verify the recipe with:

```sh
mise run vendor:check
mise run vendor:check -- gpui
```

The checker verifies archive hashes, extracts into temporary storage, applies
patches in order, and compares file contents, executable bits, and symlink targets.
It includes hidden files and ignores empty directories, which Git cannot track.
It checks crate identities, archive VCS metadata, and Cargo override paths.
It never repairs or overwrites vendored source. An active edit session fails this
check even when no source has changed yet.

Archives come from the verified local cache or Cargo's registry archive cache.
Missing archives are fetched from the pinned HTTPS URLs and cached under
`.native/vendor/archives`. A checksum mismatch fails; explicitly remove a corrupt
cached archive before downloading it again. CI's Dependencies job, `mise run
verify`, and the vendor pre-commit check enforce reproduction.

## Editing a patch

Agents own this workflow as part of an authorized fix. Choose the patch that owns
the behavior, resolve routine conflicts, and complete verification without asking
the user to operate the patch tools or approve bookkeeping steps.

1. Read the crate's manifest entry and patch descriptions. Inspect `vendor:status`
   before starting or resuming work. Start a session before editing source:

   ```sh
   mise run vendor:status
   mise run vendor:start -- libghostty-vt-sys cpu-target
   ```

2. Edit the real vendored directory and run the affected builds and tests normally.
   All patches remain applied there. Repeat the edit/build/test cycle as needed;
   there is no patch manipulation between builds. Keep edits confined to the
   selected fix. A session records all changes in that crate, including additions,
   deletions, binary files, and executable bits.
3. Once the fix passes its behavioral checks, fold those edits into its patch:

   ```sh
   mise run vendor:finish -- libghostty-vt-sys
   ```

4. Inspect the resulting patch diffs and run `mise run vendor:check`, followed by
   `mise run verify` before handoff. Commit the source and recipe together when
   committing is authorized. Do not leave an active session at handoff.

`start` creates a private Git repository under `.native/vendor/sessions`, commits
pristine upstream and each existing patch, and leaves the build tree unchanged.
`finish` captures the tested build tree, folds its edit delta into the selected
patch, and replays later patches. Earlier patches remain byte-for-byte unchanged.
Later patches are regenerated when their diff changes. Final verification applies
the generated patch text and requires an exact match with the captured build tree
before replacing patch files. The private commits are local bookkeeping and never
enter Huterm's history.

### Conflicts and interrupted work

`vendor:status` prints the active phase, target, workspace, and next command.
When replay conflicts, the tool exits unsuccessfully with the workspace path and
instructions. This is work for the agent, not a request for user approval.

- Open the conflicting files under the printed workspace's `tree/` directory.
  Inspect the original patch, the tested vendored source, and Git's conflict
  stages. Preserve the selected fix and each later patch's intent. Resolve and
  stage the files there, then run `mise run vendor:continue -- <crate>` from Huterm.
  Do not manually rebase, reset, commit, or cherry-pick in the private repository.
- If a resolved stack differs from the tested tree, the tool stops before writing
  patches. Correct the current replay result in the private `tree/` and continue.
  Review ownership as well as final behavior; do not move an entire earlier fix
  into the last patch merely to make the comparison pass. Cancel and start with
  `--adopt-edits` if the original patch choice was wrong.
- To do more edit/build/test work after starting finish, run `mise run
  vendor:reopen -- <crate>`. It retains a replay backup, returns to the editing
  phase, and preserves current vendored edits. Run finish again after testing.
- After interruption, use status and the indicated continuation command. State
  records completed replay steps and pending patch publication, so continuation
  can recover when Git committed or some patch files were written before exit.
- `mise run vendor:cancel -- <crate>` preserves source edits and moves session
  data to the printed backup path. If patch publication had started, it restores
  the original patch files first. It does not discard unrelated recipe edits.
  Canceled and reopened backups can be removed after recovery is complete.

Do not edit the manifest or patch files during a session. Recipe changes stop
finish rather than being absorbed. Preserve and reconcile any concurrent edits.
Commands use a per-crate OS lock; only one command may mutate that session at a
time. Sessions are local to a worktree. Run session commands on the host; the Linux
runner copies the fully patched build tree but excludes host `.native` state.

If source edits predate a session, inspect their diff and establish that they
belong to the authorized fix. Then explicitly adopt them:

```sh
mise run vendor:start -- gpui x11-file-drop --adopt-edits
```

The recipe still defines the baseline. Adoption assigns the existing source delta
to the named patch; it does not accept unrelated changes or rewrite the recipe.
Use the same option after canceling if you need to retarget preserved edits.
Never adopt unexplained drift merely to make verification pass. Ask the user only
when the intended behavior or ownership of unrelated work cannot be established.

## Adding a patch or upgrading a crate

For a new independent fix, add a named empty patch file and its manifest entry at
the end of the series, then start a session targeting it. The empty patch changes
nothing until finish records the implementation. Insert earlier only when a later
patch needs that dependency. Document the reason and any upstream reference.

For an upgrade:

1. Obtain the new published archive and record its checksum and VCS metadata.
   Use the release archive, not a Git checkout. Packaging can change files, and
   GPUI's current release records a dirty checkout.
2. Extract it into the new versioned vendor directory and update the manifest and
   Cargo override. Preserve the old recipe in a temporary directory while working.
   Start the new manifest entry with an empty patch list and verify the baseline.
3. For each old patch still needed, add a named empty patch and start its session.
   Apply the old patch to the new vendored tree, resolve conflicts, and finish to
   record the migrated fix. Follow the old dependency order; omit fixes supplied
   upstream. Validate the full result with the affected native tests. A clean
   textual patch application does not establish behavioral compatibility.
4. Update dependency versions and `Cargo.lock` as needed, remove the superseded
   source and patch files, update the provenance notes, and run `mise run verify`.
   Check both macOS and Linux for GPUI changes. Preserve libghostty's binding,
   native-source, and toolchain compatibility constraints.

When upstream includes every required fix, remove the override, vendored source,
patches, and manifest entry, then validate the registry dependency instead.

## libghostty-vt-sys CPU backport

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

The safe wrapper remains registry version 0.2.1 with default features disabled.
The `zig-016` patch backports upstream commit
[`6111c4d72f11f0a1894cf3c5943b9ee25a6d0c61`](https://github.com/Uzaaft/libghostty-rs/commit/6111c4d72f11f0a1894cf3c5943b9ee25a6d0c61):
native Ghostty advances to `20c3eae04dee606349eb21e2dd0293b203d47179` with
matching generated bindings and Zig 0.16.0. This includes the later upstream
[`memset` C ABI fix](https://github.com/ghostty-org/ghostty/commit/20c3eae04dee606349eb21e2dd0293b203d47179).
The initial migration pin corrupted Rust hash-table control bytes when `memset`
received a negative fill value. The public VT headers are unchanged between the
migration and fixed pins. That native revision includes Xcode
27 compatibility headers and native Apple linking support. Preserve the other
three patches, which address independent build and packaging behavior.

The `build-source-staging` patch creates a fresh private source copy under
`OUT_DIR` before invoking Zig. Zig 0.16 writes mutable `zig-pkg` dependencies
beside `build.zig`, so building in the verified source would invalidate its hash.
The copy omits Git metadata, dereferences file symlinks, and sets the Zig child's
Git discovery ceiling at canonical `OUT_DIR`, clearing inherited Git repository
overrides. Reject overlapping source and staging paths before removing previous
build output. Compilation caches stay outside the refreshed copy. The build-script
tests run through `test:build-toolchain`
and the scripting suite.

The wrapper's Kitty graphics feature must remain disabled for this backport.
Its temporary-file accessors use the old boolean ABI; the new native API expects
a directory string. Those accessors are not compiled in Huterm. Revisit this
constraint when upgrading to published matching wrappers.

The root Cargo patch selects this local crate. The build-script change causes
Cargo to rebuild native artifacts without deleting the rest of the Cargo cache.

Remove this directory and the Cargo patch when a reviewed published release
provides this CPU fix and is compatible with the selected native revision and
Zig toolchain. Update binding, native-source, and license pins together if that
release requires an API change. Keep the forced baseline CPU contract.

## GPUI X11 native file-drop correction

`gpui-0.2.2` contains the published Apache-2.0 registry crate. The archive's
SHA-256 is `979b45cfa6ec723b6f42330915a1b3769b930d02b2d505f9697f8ca602bee707`.
Its published VCS metadata names `69e2130295c2649963eb639fc70b4f2ee8ea1624` and
marks that upstream checkout dirty. The registry archive is the reproducible
source of truth. All published files are retained; Cargo's `.cargo-ok` extraction
marker is omitted.

Huterm's patch is restricted to X11 native file-drop sequencing and complete
URI-list decoding. Type negotiation selects `text/uri-list` wherever it appears
in inline offers or `XdndTypeList`; text-only offers are refused. Native
selection conversion uses a temporary requestor
window per drag. Late replies from canceled drags cannot be mistaken for a
new drag in the same Huterm window. No synthetic Pending/Submit event reaches
terminal mouse handlers before a valid Entered event. Invalid or truncated
native lists are refused as a whole. App-specific path quoting, paste admission,
and status feedback remain in Huterm.

Keep upstream formatting in this directory. Remove the Cargo patch and vendored
crate once a reviewed published GPUI release supplies equivalent native
sequencing, whole-payload validation, and stale-reply isolation. Rerun both
platforms' desktop integration smokes when removing it.

## GPUI explicit float literals

`explicit-f32-literals` adds `f32` suffixes to the two grid-track literals in
`src/taffy.rs`. Rust already infers these values as `f32`, but warns that this
fallback will become an error. Explicit types preserve the existing behavior.
Remove this patch when the selected upstream release supplies explicit types or
otherwise removes the `float_literal_f32_fallback` warnings at this call site.

GPUI 0.2.2 called `map_window()` even for `WindowOptions { show: false }`.
The hidden-window patch gates that call on `show` and propagates mapping errors.
The X11 handle patch implements `HasWindowHandle` for live XCB windows instead
of panicking. Quake uses that handle to address the exact window. The native
quake smoke checks an untouched hidden GPUI window before any hide operation,
then summons a real profile through the OS shortcut and reads its native state.
GPUI's X11 window destruction also stopped the event loop when its last window
closed. The application-lifetime patch leaves that decision to Huterm's existing
close/quit coordinator. The native smoke proves that active global registrations
retain a zero-window process, and that final-window close without registrations
still exits it.

GPUI's macOS display-link setup dereferenced `NSWindow.screen` while an animated
window was fully offscreen. AppKit returns nil in that state. The offscreen-display
patch stops the link until a screen or visibility callback restarts it, and treats
an offscreen window as not maximized. It reads backing scale from NSWindow, which
retains the real scale without a screen. A synthetic offscreen scale can resize
Metal's drawable at 2x while GPUI returns to 1x after moving onscreen. The native
quake smoke checks retained resize, drawable/viewport agreement, and PTY geometry.

The `macos-offscreen-frame` patch adds a per-window `gpuiAllowsOffscreenFrame` opt-in
and `setGpuiAllowsOffscreenFrame:` setter. It defaults to false. Only opted-in windows
bypass `constrainFrameRect:toScreen:`; other windows retain AppKit constraints.
Huterm enables it throughout quake presentation and disables it on regular
conversion and retained-window cleanup. AppKit clamps intermediate top-edge
animation frames even with a zero borderless style mask. The native smoke checks
actual slide intermediates and restored constraints on regular conversion.

## macOS exclusive global shortcuts

`global-hotkey-0.8.0` is the published Apache-2.0 OR MIT crate, with one Carbon
registration flag changed to `kEventHotKeyExclusive`. Carbon's default permits
several applications to register the same shortcut and can accept registrations
that receive no events. Exclusive registration lets Huterm report conflicts and
retain the previous working configuration instead of claiming an unusable grab.

The archive SHA-256 is
`8c386b0a4a70cb2d39fffd74480f985b6f0bfbcb934b6a6b6b7e630e448f242e`; its upstream
revision is `2a620bf3852008b568f6d36c2baedcc3dd0822f2`. All other published files
are unchanged. Remove this patch when a reviewed release exposes exclusive
registration and Huterm selects it. The native quake smoke holds the shortcut
in a separate process and checks startup and reload rejection.
