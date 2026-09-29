# Vendored dependencies

## Reproducing local changes

[sources.json](sources.json) pins each published crate archive by URL and SHA-256,
records its upstream VCS metadata, and lists its patches in application order.
Each patch has a stable name, a description, and an upstream link when available.
Keep each coherent fix together. GPUI's core crate has hidden-window creation,
file-drop pointer-modality, and scene sprite-order patches, its Linux crate has
X11 file-drop, native-handle, and fullscreen-state patches, and its macOS crate
has offscreen-screen and per-window frame-constraint patches.

Normal Cargo builds use the fully patched vendored source through
`[patch.crates-io]`. They do not apply patches. Verify the recipe with:

```sh
mise run vendor:check
mise run vendor:check -- gpui-pre-linux
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
   mise run vendor:start -- gpui-pre-linux x11-file-drop
   ```

2. Edit the real vendored directory and run the affected builds and tests normally.
   All patches remain applied there. Repeat the edit/build/test cycle as needed;
   there is no patch manipulation between builds. Keep edits confined to the
   selected fix. A session records all changes in that crate, including additions,
   deletions, binary files, and executable bits.
3. Once the fix passes its behavioral checks, fold those edits into its patch:

   ```sh
   mise run vendor:finish -- gpui-pre-linux
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
mise run vendor:start -- gpui-pre-linux x11-file-drop --adopt-edits
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
   Use the release archive, not a Git checkout. Packaging can change files.
   `gpui-pre` archives have no Cargo VCS file; mark their manifest entries with
   `"snapshot": "gpui-pre"` so the checker reads the Zed crate and revision from
   `package.metadata.gpui-pre` instead.
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
   Check both macOS and Linux for GPUI changes.

When upstream includes every required fix, remove the override, vendored source,
patches, and manifest entry, then validate the registry dependency instead.

## GPUI snapshot crates

Zed has not published GPUI to crates.io since 0.2.2. Huterm uses `gpui-pre`
0.3.6, which republishes Zed's workspace crates from revision
`bcf6582ce3500df93a8a39366640173e6786cea6` as separate Apache-2.0 registry
crates. Only the three patched crates are vendored: `gpui-pre` (Zed's
`crates/gpui`), `gpui-pre-linux` (`crates/gpui_linux`), and `gpui-pre-macos`
(`crates/gpui_macos`). The other snapshot crates resolve from the registry.
The archives omit Cargo's VCS file and record their Zed crate and revision in
`package.metadata.gpui-pre`, which `vendor:check` compares with the manifest.
The registry archive is the reproducible source of truth. All published files
are retained; Cargo's `.cargo-ok` extraction marker is omitted. Keep upstream
formatting in these directories.

Upstream supplies fixes that earlier Huterm patches carried: the buffered X11
event drain (zed#62081), app-owned macOS title-bar drags (zed#41839,
zed#60620), sharp zero-blur shadows in the wgpu renderer (zed#57685), the
borderless traffic-light guard, explicit application lifetime, a live X11 window
handle, a patched Taffy grid, and explicit float literals.

## GPUI X11 native file-drop correction

Huterm's `x11-file-drop` patch is restricted to X11 native file-drop sequencing
and complete URI-list decoding. Type negotiation selects `text/uri-list` wherever
it appears in inline offers or `XdndTypeList`; text-only offers are refused.
Native selection conversion uses a temporary requestor
window per drag. Late replies from canceled drags cannot be mistaken for a
new drag in the same Huterm window. No synthetic Pending/Submit event reaches
terminal mouse handlers before a valid Entered event. Invalid or truncated
native lists are refused as a whole. App-specific path quoting, paste admission,
and status feedback remain in Huterm.

Remove the patch once a reviewed GPUI snapshot supplies equivalent native
sequencing, whole-payload validation, and stale-reply isolation. Rerun both
platforms' desktop integration smokes when removing it.

GPUI suppresses hitbox hover while the last input was a key press, and only
mouse and touch events end that state. Native file drags arrive as `FileDrop`
events without mouse events, so a drop after typing found no target and was
discarded. The `file-drop-pointer-modality` patch counts drag entry, movement,
and drop as pointer input. The integration smoke types a barrier key after
positioning the pointer and before each native drop, so it fails without it.

## GPUI window lifetime and native state

GPUI still calls `map_window()` even for `WindowOptions { show: false }`.
The `hidden-window-creation` patch gates that call on `show` and propagates
mapping errors. The `x11-window-handle` patch reports a destroyed X11 window as
unavailable instead of returning its stale Xcb handle. Quake uses that handle to
address the exact window. The native quake smoke checks an untouched hidden GPUI
window before any hide operation, then summons a real profile through the OS
shortcut and reads its native state. Upstream X11 window destruction no longer
stops the event loop; GPUI's `QuitMode` now decides instead, and Huterm selects
`QuitMode::Explicit` so its close/quit coordinator owns zero-window lifetime.
The native smoke proves that active global registrations retain a zero-window
process, and that final-window close without registrations still exits it.

The `fullscreen-state-notification` patch notifies bounds observers when an X11
window manager changes `_NET_WM_STATE` fullscreen without a geometry event.

GPUI's macOS display link now tolerates a nil `NSWindow.screen` while an
animated window is fully offscreen. The `macos-offscreen-screen` patch also
treats such a window as not maximized, and reads backing scale from NSWindow,
which retains the real scale without a screen. A synthetic offscreen scale can
resize Metal's drawable at 2x while GPUI returns to 1x after moving onscreen.
The native quake smoke checks retained resize, drawable/viewport agreement, and
PTY geometry.

The `macos-offscreen-frame` patch adds a per-window `gpuiAllowsOffscreenFrame` opt-in
and `setGpuiAllowsOffscreenFrame:` setter. It defaults to false. Only opted-in windows
bypass `constrainFrameRect:toScreen:`; other windows retain AppKit constraints.
Huterm enables it throughout quake presentation and disables it on regular
conversion and retained-window cleanup. AppKit clamps intermediate top-edge
animation frames even with a zero borderless style mask. The native smoke checks
actual slide intermediates and restored constraints on regular conversion.

## GPUI scene sprite order

`Scene::finish` sorted each sprite list by draw order and atlas tile ID, but
batching only breaks on draw order and atlas texture. The tile key made every
frame reorder thousands of glyph sprites of about 100 bytes each, even when a
layer's sprites already shared one texture. The `scene-sprite-texture-order`
patch sorts by draw order and texture index instead; each sprite list holds one
texture kind. The stable sort keeps insertion order within a texture and finds
an already ordered list in one pass. On macOS, sampling a full-grid terminal
paint put `Scene::finish` at 697 of 1,274 `Window::draw` samples before the
patch and 18 of 720 after it. The renderer smoke's held fixture differed only
in three pixels by 1/255. Remove the patch if upstream changes the sort key.

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

## portable-pty child descriptor limit

`portable-pty-0.9.0` is the published MIT crate from WezTerm's `pty` directory.
The archive SHA-256 is
`b4a596a2b3d2752d94f51fac2d4a96737b8705dddd311a32b9af47211f08671e`; its upstream
revision is `f8921727a11b9f8b073e8c24821d72fd41283500`. All other published
files are unchanged.

The `child-nofile-limit` patch adds a Unix-only `CommandBuilder::nofile_limit`
setter that stores an optional soft and hard `RLIMIT_NOFILE` pair, like the
existing `umask` field. The Unix `pre_exec` closure applies it with
`setrlimit` after `close_random_fds` and before the umask, and ignores failure,
as Ghostty does. This lets Huterm raise its own soft limit while children keep
the limit it started with; see
[the descriptor limits plan](../../docs/plans/descriptor-limits.md). The crate
exposes no safe pre-exec hook, and Huterm denies `unsafe_code`, so the
child-side call lives here. `pty::tests::child_starts_with_the_configured_open_file_limit`
in huterm-core checks the child's soft limit. Remove the patch when a reviewed
release offers an equivalent option.
