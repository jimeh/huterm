# Settings UI and safe config writes

Status: in progress. Approved on 2026-10-26 after four Codex review rounds;
PR 1 (the foundation) is in progress. Plans [issue #35][issue-35] and
[issue #34][issue-34] within [epic #14][issue-14]. The UI decisions come from
two rounds of HTML mockups ([round two][mockups], [the chosen
layout][mockup-a1]).

Huterm gets a Settings window that edits `config.toml` in place. The file stays
the source of truth: Settings changes only the lines a setting owns, keeps
comments and formatting, notices edits made in other editors, and never
overwrites them. Huterm also starts watching the file and reloads it when it
changes.

PR 1 delivers the foundation without the window: comment-preserving patches,
external-edit protection, file watching with `[general] watch_config`, and
per-binding `disabled = true` keybindings. Later PRs build the window on it.

## Decisions

These were settled with the user and are not open for re-litigation without
new evidence.

- Settings is a separate window, not an overlay inside a terminal window like
  About. It is a new window kind with no session, workspace, or terminal.
  `open_settings` focuses the existing window instead of opening a second.
- The layout follows mockup variant A1: a sidebar of sections (General,
  Appearance, Tabs, Window and panes, Terminal, Scrollback, Command palette,
  Quake windows, Keybindings, and Updates on macOS) and grouped cards of rows.
  The window's colors derive from the active theme, as overlays do through
  `Swatch`.
- Edits save immediately. There are no notices for individual saves. File
  status ("Saved", "Reloaded", "config.toml changed on disk", errors) and an
  "Open config.toml" link sit in the sidebar footer.
- Keybindings list commands whose rows expand in place. Each command can have
  several shortcuts. Each shortcut has its own `when` condition, arguments, and
  an enable switch. Quake profile arguments use a dropdown, as does the profile
  picker in the Quake windows section, because segmented tabs do not fit more
  than two profiles.
- A `[[keybinding]]` entry with `disabled = true` turns off earlier bindings
  with the same key, command, and `when`, and binds nothing itself. Editing or
  switching off a default writes such an entry, plus a replacement when the
  key or condition changes. Settings never writes a whole-key `unbind`.
- Huterm watches `config.toml` and reloads it when it changes, controlled by
  `[general] watch_config`, which defaults to `true`. Settings shows the
  setting in its General section.
- Hubris's settings manager (`apps/server/src/settings_manager.rs` and
  `keybindings_manager.rs`) is the reference for `toml_edit` patching, atomic
  writes, and watching. Its gaps are fixed here, not copied: it writes
  keybindings back by position, so deleting a middle entry moves later
  entries' comments; replacing a value drops its trailing comment; renaming
  over a symlink replaces the link; and nothing re-reads the file before a
  write.

## PR 1: foundation

### Objective

A user who edits `config.toml` in any editor sees Huterm apply the change
without pressing Reload, and can turn that off. A user can switch off one
default shortcut without losing the other bindings on its key. The crate has a
tested service that changes individual settings and keybindings in the file
without disturbing anything else and without overwriting concurrent edits.

### Constraints

- `huterm-config` stays independent of GPUI, core, and terminal engines. The
  patch service lives there so a future server or text client can use it.
- Blocking file I/O, hashing, parsing, and watcher start, reconfiguration,
  and drop stay off the GPUI thread. `notify`'s FSEvents backend blocks while
  starting and joins a thread when dropped. Reload keeps its current
  background-executor path.
- An idle watcher costs nothing: no armed timers, no periodic reads.
- macOS and Linux both ship. Watching works with native events on both, and
  with symlinked configuration files, a common dotfiles arrangement.
- Startup behavior is unchanged. Malformed TOML and fatal terminal-policy
  errors still stop startup, as the desktop client guide requires, so the
  watcher only ever recovers from errors that let Huterm start.
- New and changed dependencies follow the release-age policy and pass
  `mise run license`. `toml_edit` 0.25.17 is already in `Cargo.lock`
  transitively; pin it directly. Add `notify` 8.2.0 (CC0-1.0, allowed) for
  watching, pinned.
- Schemas regenerate through `mise run schema:generate` and stay in sync with
  runtime parsing through `schemas/fixtures.json`.

### Configuration contract

`[general]` is a new table, matching Settings' General section:

```toml
[general]
# Reload this file automatically when it changes on disk.
watch_config = true
```

- `GeneralConfig { watch_config: bool }` in `huterm-config`, defaulting to
  `true`, with `deny_unknown_fields` and a schema description.
- `RawConfig` gains `general`. The desktop `Config` carries it.
- The default config template gains the table at the top, with its comment.

`[[keybinding]]` entries gain `disabled`:

```toml
# Keep cmd-h for the shell; Hide stays in the menu.
[[keybinding]]
key = "cmd-h"
command = "hide"
disabled = true
```

- A disabled entry removes every earlier binding, default or user, whose
  keystrokes, command, and combined predicate equal its own. The combined
  predicate is the command's catalog context joined with `when`, exactly as
  same-key replacement builds it today, so `Terminal&&selection` and
  `Terminal && selection` match. When the entry has `args`, an earlier binding
  must also have equal arguments; omitted `args` match any arguments.
- It binds nothing: compilation drops it before same-key replacement,
  reservation, and dispatch ordering. A removed binding therefore releases
  its reserved keystroke and its menu accelerator unless another binding
  still claims them. The command and any arguments are still validated, so a
  typo fails the reload like any other bad entry.
- Order matters as it does for other entries. A disabled entry affects only
  bindings before it. A later entry can bind the same key again, and a later
  `unbind` still removes whatever remains.
- A disabled entry that matches nothing is silent. One dotfile often serves
  both platforms, and a macOS-only default has nothing to match on Linux.
- `disabled = false` is the same as omitting the field on ordinary entries.
- Any `disabled` key, true or false, is an error on `unbind` and on
  `[[global_keybinding]]`, so parsing keeps the field's presence
  (`Option<bool>`). Enabling and disabling global shortcuts is a later
  Settings concern.
- The schema adds a boolean `disabled` to keybindings, forbids it for
  `unbind`, and omits it from global keybindings, matching the runtime.

### Patch service (#34)

A `huterm_config::document` module changes the file. Typed validation and
source preservation stay separate: `toml_edit` analyzes structure, and edits
splice into the original text.

- **Paths.** `ConfigFile` keeps the configured logical path, the path the
  application reads, separately from the target it resolves to through
  symlinks. Writes go to the target, so the link survives. Theme lookup keeps
  resolving relative to the logical path, as `config.rs` does today.
- **Revision.** A revision is the content hash of the bytes read, together
  with the resolved target's pathname and its identity (device and inode on
  Unix).
- **Rendering.** The service parses with `toml_edit::Document`, which keeps
  source spans, and edits whole statements in the original text. A statement
  runs from its key through its value, including multiline values, plus the
  rest of the line and its line ending. A leading comment block, meaning the
  contiguous comment lines directly above a statement or header with no blank
  line between them, belongs to that statement or header. Ranges never
  overlap: whatever precedes a comment block ends before it. Every other byte
  of the file is copied unchanged.
  - Replacing a value swaps only the value's span, so its key, spacing, and
    trailing comment stay byte-for-byte.
  - A path defined inside a single-line inline table, such as
    `font = { family = "…", size = 18.0 }`, is edited by re-rendering only
    that inline table's value span from its parsed form, with only the
    requested field changed. Sibling fields survive, and the statement's key
    and trailing comment stay byte-for-byte. A multiline inline table that
    contains comments (TOML 1.1) returns `UnsupportedLayout` without writing,
    because re-rendering can move or drop comments the parser attaches to
    neighbouring keys. Settings will tell the user to edit the file directly.
  - A path whose table uses dotted keys, such as `font.size = 13` at the top
    level or inside another table, gets new keys as dotted statements after
    the table's last dotted statement. Removal removes that one statement.
  - A path in a `[table]` section gets new keys after the section's last
    statement. Removal removes that statement.
  - A table that does not exist yet becomes a new `[table]` section, placed
    before the first array of tables so keybindings stay at the end.

  New lines use the file's line ending: CRLF when every existing line ending
  is CRLF, LF otherwise. A file without a final newline keeps that.
- **Empty tables.** Removing a table's last key keeps the table by default,
  because an empty table can mean something: an empty
  `[quake.profiles.logs]` still defines the `logs` profile with default
  values. Only grouping tables, the fixed sections of `RawConfig` such as
  `font`, `tabs`, `general`, or `terminal.bell`, may be removed when they
  become empty with no comments, since an empty grouping table equals an
  absent one. Entries of named maps, `quake.profiles.*` and `themes.*`, are
  never removed implicitly.
- **Semantic delta guard.** After splicing, the service parses the old and
  new text into value trees and compares them with the expected tree: the old
  tree plus the requested operations plus any permitted grouping-table
  removals. Any other difference, such as a sibling that disappeared, is a
  bug: `apply` returns an error and writes nothing.
- **Comment guard.** Every comment in the old text, except those inside a
  removed statement, entry, or permitted table removal, must appear unchanged
  in the new text. A missing comment is a bug, handled the same way.
  Exact-output fixtures and both guards together cover layouts the rules
  above might mishandle.
- **Patch operations:**
  - set a scalar or inline value at a dotted path, such as `font.size` or
    `quake.profiles.logs.height`;
  - remove a key, which is how reset-to-default works;
  - append a keybinding entry; and
  - update, set or clear one field of, or remove a keybinding entry, located
    as described next.
- **Entry locators.** A locator holds the entry's index and its full content
  in the base snapshot: every field, including `args`, `description`, and
  `disabled`, parsed and normalized. Against the current file, the locator
  resolves to the entry at the same index when its content is unchanged,
  else to the single entry anywhere with that exact content. No match, or
  more than one, is a `Conflict`. An unchecked index or the first match on a
  partial tuple is never used. An entry owns its leading comment block, its
  header and statements, and each of its own subtable sections such as
  `[keybinding.args]`, even when unrelated sections intervene. Each owned
  section ends before the next header or that header's leading comment block,
  whichever comes first. Removing an entry removes these non-overlapping owned
  ranges and at most one adjacent blank separator line per range, preserving
  unrelated sections. Changing one field edits only that field's source
  ranges, leaving the entry's other fields and comments in place.
- **`apply(base, patches, validate)`:**
  1. Resolve the logical path and read the current target.
     - A missing file is created; the base must say it was absent.
     - Missing parent directories are created; step 7 makes their entries
       durable.
     - A dangling symlink is an error, since writing would replace the link
       or create a file the user did not expect.
  2. Validate the current text with `validate`, the same check as the
     candidate gets. A syntax error or a configuration error, such as
     `tabs.min_width` above `tabs.max_width`, returns `InvalidFile` and
     writes nothing.
  3. When the current revision differs from the base, rebase. A
     path-addressed patch applies when its path has the same value in the
     base and the current file. An entry patch applies when its locator
     resolves. Otherwise the whole call fails with `Conflict`, naming each
     path or entry.
  4. Splice the patches into the current text and run `validate` on the
     candidate. A rejection writes nothing.

     The desktop's `validate` is side-effect free. It runs the full parse,
     value validation, theme resolution, local keymap compilation, and global
     keybinding validation without registering anything.
  5. Write the candidate to a temporary file in the target's directory, with
     the target's permissions, and fsync it.
  6. Immediately before renaming, re-resolve the logical path. Check that it
     resolves to the same pathname, that the file there has the same
     identity, and that it has the same content. Comparing pathnames catches
     a link retargeted to a hard link of the original. Any change deletes the
     temporary file and restarts once from step 1, which turns a concurrent
     edit into a rebase or a `Conflict`. A second change returns `Conflict`
     without renaming.
  7. Rename over the target. On Unix, fsync the target's directory, then the
     parent of each directory this call created, innermost first.
- **Failures.** A failure before the rename deletes the temporary file and
  leaves the original intact; directories it created may remain. A failure
  after the rename, such as any directory fsync error, returns
  `WrittenNotDurable`: the replacement happened, and the
  caller refreshes its revision from disk. The service never rolls back,
  because rolling back could overwrite a later external edit.
- **Return value.** `apply` returns the new text and revision. Only a
  persisted and validated patch reaches the effective settings, and only
  through the reload that the new revision triggers.
- **Temporary files** are named `.<file>.huterm-tmp-<pid>-<counter>`. Each
  write removes files with that prefix that are more than ten minutes old, so
  a crashed save gets cleaned up without touching another process's write in
  progress.
- **Synchronization.** Callers serialize `apply` through the owner of the
  `ConfigFile`, one writer per process. Another process can still write
  between the check in step 6 and the rename; that window is accepted and
  documented in the code.
- **Callers.** Nothing in the desktop calls the service yet. The Settings
  window in PR 2 is its first caller.

### Watching

A `config_watch` module in `huterm-gpui` owns a `notify` watcher on a
background thread. That thread starts it, reconfigures it, and drops it.

- **What it watches.** The logical path's directory, non-recursively, and the
  resolved target's directory when the file is a symlink. It also watches the
  directory of every external theme file the last load read or probed, which
  usually means `themes/` beside the logical path. A required directory that
  does not exist yet, such as `~/.config/huterm` on a fresh profile, is
  covered by watching its nearest existing ancestor. Registration moves down
  as directories appear and back up when they are removed.
- **Changing registrations.** Only registrations whose topology changed are
  touched. The ancestor watch stays installed until the descendant's
  registration succeeds, so coverage never lapses. A `NotFound` while
  registering, from a directory removed in between, keeps the existing
  watches and resolves the topology again. Each actual registration change
  schedules one catch-up reconciliation; a failed or unchanged registration
  schedules none, so reconciliation cannot loop.
- **Event filtering.** Read and open access events are ignored. Hashing opens
  the file, so reacting to them would loop forever on inotify. Creations,
  modifications, renames, and removals that touch a watched input schedule
  work. Rescan or overflow notifications count as a change even without
  paths.
- **Fingerprint.** Every load and reload records a fingerprint of all its
  inputs, hashed from the bytes that load actually consumed: the
  configuration file's content, plus the existence and content of each
  external theme path that theme resolution read or probed, along the whole
  inheritance chain. A theme edit therefore changes the fingerprint even when
  `config.toml` does not.
- **Reconciling.** Reconciliation has two steps, kept apart so content
  equality never blocks a topology change:
  1. resolve the topology, meaning the symlink target and dependency
     directories, and install the watches it requires; then
  2. recompute the fingerprint off the GPUI thread and reload when it differs
     from the last *attempted* reload's fingerprint.

  Comparing against attempts rather than successes means that restoring
  known-good content after a broken save reloads and clears the failure
  notice. Reconciliation runs after a 150 ms quiet period following events,
  at startup once the watches are installed, and after every registration
  change. The last two cover changes made while no watch was installed,
  including FSEvents' restart when watches change. A symlink retargeted to
  identical content still moves its watch.
- **Transient misses.** A `NotFound` while an editor saves by renaming skips
  that round without a notice. The next event reconciles.
- **Reload coalescing.** The existing `reloading` latch gains a dirty flag.
  An event during an in-flight reload marks it dirty, before any dedupe, and
  one more reload runs when the current one finishes. Manual Reload always
  reloads, regardless of fingerprints.
- **Turning watching on and off.** After every reload, successful or not, the
  watcher starts, stops, or reconciles to match the effective
  `general.watch_config`. A failed reload keeps the previous value. Turning
  watching on also reconciles once, catching changes made while it was off.
- **Startup.** A nonfatal startup error falls back to defaults, as today.
  `watch_config` is then read leniently from the valid TOML, as the
  clipboard policy is, defaulting to `true`, so a fix saved in an editor
  loads without a manual Reload.
- **Idle cost.** With no events, there is no timer and no file access.
- **Notices.** Failures keep today's behavior: an automatic reload of an
  invalid file shows the persistent configuration notice with its Reload
  action, and keeps the last valid settings. A successful automatic reload
  replaces the diagnostics, as manual reload does, and adds no notice of its
  own. A watcher that cannot start shows one warning saying `config.toml`
  will not reload automatically. Manual Reload still works.
- **Smoke visibility.** Smoke state output reports the watcher's state,
  `watching=on` or `watching=off` once the worker has acknowledged the
  change, and a count of reconciliations. Native tests wait on it before
  writing.

### Files

- `crates/huterm-config/src/lib.rs`: `GeneralConfig`, `RawConfig.general`,
  and `RawKeybinding`/`KeybindingEntry` `disabled` as `Option<bool>`, plus
  its validation.
- `crates/huterm-config/src/document.rs`, with tests in a child module: the
  patch service.
- `crates/huterm-config/src/schema.rs` and `schemas/*`: the `general` table,
  `disabled`, and the unbind and global rules. Also `schemas/fixtures.json`.
- `crates/huterm-gpui/src/keymap.rs` and its tests: disabled resolution.
- `crates/huterm-gpui/src/themes.rs`: report the external theme paths that
  resolution read or probed, for the fingerprint.
- `crates/huterm-gpui/src/config.rs`: the template, the lenient
  `watch_config` read, the input fingerprint on load and reload, and a
  side-effect-free validator for the patch service.
- `crates/huterm-gpui/src/desktop/config_watch.rs` (new) and
  `desktop/windows.rs`: the watcher's lifecycle, the dirty flag on reload,
  notices, and smoke state.
- Global keybinding validation, wherever it rejects `when` today.
- `Cargo.toml`, `Cargo.lock`, and `crates/*/Cargo.toml`: `toml_edit` and
  `notify`.
- `scripts/check-native-input.ts`, `scripts/check-linux-input.ts`, and the
  input smoke fixtures: automatic reload steps.
- `docs/configuration.md`: Reload becomes automatic, with `[general]`.
- `docs/keybindings.md`: `disabled`, and how it differs from `unbind`.
- `docs/agents/desktop-client.md`: the watcher's threading, event filtering,
  and reload coalescing rules.

### Observable success

- Saving `config.toml` in an editor applies the change in open windows within
  about a second, with no notice on success.
- Saving an invalid file shows the existing configuration notice and keeps the
  previous settings. Saving a fixed file, including the exact earlier content,
  clears the notice.
- With `watch_config = false`, edits apply only on manual Reload.
- Editing a symlinked `config.toml`, the active theme file, or a theme it
  extends reloads.
- An idle Huterm with watching on does no file work.
- `disabled = true` for `cmd-h` → `hide` removes that binding only. Any other
  binding on `cmd-h` with another command or `when` remains, and the key no
  longer reaches GPUI as a reserved stroke unless something else claims it.
- The patch service changes `font.size` in a commented CRLF file, and the
  diff is that one value.

### Failure paths and risks

| Risk | Handling |
| --- | --- |
| Editor saves by delete-and-create or rename | Directory watches, quiet period, and skipping `NotFound` |
| Burst of events while reloading | Dirty flag and one follow-up reload |
| Own reads or writes trigger more work | Access events ignored; fingerprint compared with the last attempt |
| Theme change without a config change | Theme inputs are part of the fingerprint |
| Restoring good content after a failed reload | Compared with the last attempt, so it reloads and clears the notice |
| Watch backend unavailable | One warning notice; manual Reload still works |
| Concurrent manual edit during a patch | Rebase if disjoint, else `Conflict`, and no write |
| Duplicate or reordered keybinding entries | Full-content locators; ambiguity is a `Conflict` |
| File already invalid, syntactically or semantically | `InvalidFile`, and no write |
| Symlink retargeted during a save | Re-resolve; compare pathname, identity, and content before rename; restart once, then `Conflict` |
| Inline-table or dotted-key layouts | Statement-scoped edits plus the semantic delta and comment guards |
| Commented multiline inline table | `UnsupportedLayout`, and no write |
| Resetting a named profile's last key | The empty table is kept, so the profile survives |
| Config directory missing on a fresh profile | Watch the nearest existing ancestor and move down |
| Changes while watches are reconfigured | Catch-up reconciliation after every registration change |
| Failure before rename | Temporary file removed, original intact |
| Failure after rename | `WrittenNotDurable`; caller refreshes from disk; no rollback |
| Crash between temporary write and rename | Original intact; leftover removed after ten minutes |
| Line endings and final newline | Splice edits; new lines follow the file's ending |
| Two Huterm processes writing | Recheck narrows the window; residual risk documented |

### Non-goals for PR 1

- The Settings window, its controls, and any desktop caller of the patch
  service.
- Schema metadata for labels, groups, order, units, and apply semantics.
- Disabling global keybindings.
- Recovering from malformed TOML at startup, which stays fatal.
- Cross-process locking.

### Verification

Tests synchronize on observable progress. Debounce and state transitions use
a controllable clock and pure functions; races use barriers; native watcher
tests wait for registration, then poll the observed state under a generous
bounded deadline and report the last state on timeout. No fixed sleeps.

| Concern | Evidence |
| --- | --- |
| Set, remove, add key, and new table leave every other byte unchanged | `document` fixture tests comparing exact output |
| LF, CRLF, mixed endings, and a missing final newline | Fixture tests |
| Trailing comment and spacing survive a value change | Fixture test |
| Empty-table cleanup only without comments | Fixture tests |
| Removing a middle keybinding keeps the other entries' comments | Fixture test |
| Inline-table field set and reset keeps sibling fields; dotted keys; multiline values; `[keybinding.args]` subtables; unknown fields | Exact-output fixtures |
| Commented multiline inline table returns `UnsupportedLayout` and writes nothing | Fixture test |
| Removing an entry before a commented entry or commented section keeps that comment | Exact-output fixtures |
| Removing an entry with `[keybinding.args]`, including an intervening unrelated section | Exact-output fixture preserving unrelated sections and their comments |
| Last-key reset keeps a named profile; removes an empty grouping table; dotted-key layout | Exact-output fixtures |
| Semantic delta and comment guards refuse an edit that changes anything else | Tests with forced bad splices through the internal seam |
| Setting one field of an entry keeps its other fields and comments | Fixture test |
| Locator: duplicate tuples with different args, insertion, deletion, reorder | Tests resolving or returning `Conflict` |
| Disjoint external edit rebases; overlapping edit conflicts | Tests changing the file between load and apply |
| Syntactically or semantically invalid current file writes nothing | Tests asserting unchanged bytes |
| Candidate rejected by the validator writes nothing | Test |
| Failures at temporary write, file sync, recheck, rename, directory sync, and created-parent sync | Fault-injection tests through an internal file-operations seam |
| Permissions preserved; symlink written through; dangling symlink rejected | Unix tests |
| Symlink retargeted between read and rename, including to a hard link of the original | Tests with a barrier at the recheck |
| A second change during the restarted attempt | Test returning `Conflict` without renaming |
| Missing file and missing parent created | Tests |
| Stale temporary file removed; fresh one kept | Test with controlled modification times |
| `disabled` matches key, command, combined predicate, and args | `keymap` unit tests, including a contextual default and a normalized `when` |
| Omitted versus given args; disable, rebind, and `unbind` ordering | `keymap` unit tests |
| Disabled binding releases `ReservedKeys` and its menu accelerator | `keymap` tests on reserved keys and effective bindings |
| Disabled default no longer dispatches; other bindings on the key still do | GPUI dispatch test, alongside existing keymap tests |
| Unmatched disabled is silent; `disabled` on `unbind` or global, true or false, errors | Unit tests |
| Schema accepts and rejects the same cases as runtime | `schemas/fixtures.json` cases and `mise run schema:check` |
| `[general] watch_config` parses, defaults, and is read leniently on a nonfatal error | `config` tests |
| Event filter ignores access, accepts mutations, and treats rescans as changes | Pure-function tests |
| Fingerprint changes for config, active theme, parent theme, and created theme files | Tests on a temporary directory |
| Valid A, invalid B, restore A, and a held B reload followed by A | Reload state-machine tests |
| Dirty flag during an in-flight reload; disable and re-enable with queued work | State-machine tests |
| Watcher sees external, symlink-target, and theme writes on the host platform | `config_watch` tests on a temporary directory, waiting for registration |
| First run: the config directory and file are created after the watcher starts | `config_watch` test observing a reload |
| New dependency, change during re-registration, identical-content retarget | Tests with barriers around registration |
| Descendant removed just before registration, then recreated | Barrier test: ancestor stays watched, recovery observed, and the watcher goes idle |
| Automatic reload in the running app | Extend `smoke:macos-input` and `smoke:linux-input`: wait for `watching=on`, write a policy change without pressing Reload, and await the new state |
| Watching off does not reload | Controlled-clock test: deliver a mutation while disabled, advance past the quiet period, drain work, and assert unchanged settings and reload count |
| Watching turns off and on in the running app | Smoke: await `watching=off`, then check that manual Reload still applies a change; await `watching=on` after re-enabling |
| Idle cost | `config_watch` test asserting no timer and no reads without events |
| Repository gates | `mise run check`, `mise run test`, `mise run test:ghostty-releasesafe`, and `mise run license` |

## Later PRs

Each later PR gets its own contract when it starts, built on this section.

1. **Settings model and metadata.** Semantic metadata lives on the config
   types and is emitted as `x-huterm-*` schema extensions through
   `#[schemars(extend(...))]`: label, group, order, unit, apply semantics
   (live, new tabs, new terminals, later events, next palette open),
   platform, advanced, and visible-when. Constraints and descriptions now
   patched in `schema.rs` move onto the types. A typed `SettingsModel` parsed
   from the committed schema drives the UI. A test fails when a schema leaf
   has neither a generic control, an override, nor a place on an explicit
   file-only list.
2. **Window and controls.** The Settings window kind and its Close and Quit
   handling, the A1 shell, and the sidebar footer status. A control kit built
   on existing pieces (`ui/text_field.rs`, `MenuModel`, `Swatch`, `KeyHint`,
   and scrollbars): switch, segmented control, select, number field with unit
   and range, modifier chips, and form row. It also covers the scalar
   settings, reset, validation, and the theme picker with color overrides. A
   GPUI-side override table gives special controls, hides settings, or puts
   them behind Advanced.
3. **Keybinding editor.** Expanding command rows, a key recorder with a
   capture context the window keymap cannot intercept, `when` completion from
   the context list and validation, argument controls, and the enable switch
   that writes `disabled` entries.
4. **Quake profiles** with a profile dropdown, and global shortcuts.

Native keyboard and focus smokes cover navigation, save, reset, and returning
to a terminal, as issue #35 requires.

## Open questions

None for PR 1.

[issue-14]: https://github.com/jimeh/huterm/issues/14
[issue-34]: https://github.com/jimeh/huterm/issues/34
[issue-35]: https://github.com/jimeh/huterm/issues/35
[mockups]: https://plans.jimeh.dev/ebw3yvexaw6v76vs24krpasaoe/huterm-settings-round2.html
[mockup-a1]: https://plans.jimeh.dev/2pjfyytgq3lqoe5c5iduqjuzlm/huterm-settings-a1-revised.html
