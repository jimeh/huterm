# Vendored dependencies

## Reproducing local changes

[sources.json](sources.json) pins each published crate archive by URL and SHA-256,
records its upstream VCS metadata, and lists its patches in application order.
Each patch has a stable name, a description, and an upstream link when available.
Keep each coherent fix together. The GPUI file-drop correction is one coupled
fix; the sys crate has separate CPU, build-script watch-path, and license patches.

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

The safe wrapper remains registry version 0.2.1, native Ghostty remains
`a887df42c56f6de86c0fe6da9c4eeca37931e083`, and Zig remains 0.15.2. No current
upstream bindings or native API changes are included. The root Cargo patch
selects this local crate; its source identity invalidates old registry-built
native artifacts without deleting the rest of the Cargo cache.

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
