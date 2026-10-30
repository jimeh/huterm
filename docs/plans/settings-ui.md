# Settings UI and safe config writes

Status: implemented. Approved on 2026-10-26 after four Codex review rounds.
PR 1 (the foundation) shipped in #263, PR 2 (the settings model) in #266,
PR 3 (the window and controls) in #269, and PR 4 (the keybinding editor)
in #274; PR 5 (quake profiles and global shortcuts) completed the series.
Plans
[issue #35][issue-35] and [issue #34][issue-34] within
[epic #14][issue-14]. The UI decisions come from two rounds of HTML mockups
([round two][mockups], [the chosen layout][mockup-a1]).

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

## PR 2: settings model and metadata

### What PR 2 delivers

Settings metadata lives on the config types and reaches the committed schema
as `x-huterm-*` extensions. A typed `SettingsModel`, parsed from that schema,
lists every setting the window will show, in order, with what a control needs.
A coverage test keeps the schema, the model, and the GPUI override table in
step. Nothing user-visible changes except the published schema's new keys.

### Settings metadata

Every setting carries its metadata on the field that defines it, through
`#[cfg_attr(feature = "schema", schemars(extend(...)))]`:

| Key | Value | Required |
| --- | --- | --- |
| `x-huterm-section` | `general`, `appearance`, `tabs`, `window`, `terminal`, `scrollback`, `palette`, `quake`, `keybindings`, or `updates` | yes |
| `x-huterm-group` | Card title within the section, such as `"Font"` | yes |
| `x-huterm-label` | Row label, sentence case | yes |
| `x-huterm-order` | Integer, unique within its section | yes |
| `x-huterm-apply` | `live`, `new_tabs_and_panes`, `new_terminals`, `later_events`, or `next_palette_open` | yes |
| `x-huterm-unit` | Display unit, such as `"pt"`, `"rows"`, `"s"`, `"ms"`, `"h"` | no |
| `x-huterm-platform` | `macos` or `linux` | no |
| `x-huterm-advanced` | `true` | no |
| `x-huterm-visible-when` | `{"setting": "<path>", "equals": <value>}` | no |
| `x-huterm-zero-label` | Text a slider shows at 0, such as `"Off"` | no |

Enum variants carry `x-huterm-label` too, so a select shows "Title bar"
rather than `titlebar`. A variant's doc comment stays its description.

A setting is a schema property that carries `x-huterm-label`. An object
property without one is a namespace, such as `tabs` or `terminal.bell`, and
the model recurses into it. Compound settings carry their own label at the
`RawConfig` field: `theme`, `themes`, `keybinding`, `global_keybinding`, and
`quake`. Their inner fields get metadata in the PRs that build their editors.

Apply semantics follow the runtime, checked against `docs/configuration.md`
and the reload path: `terminal.term` applies to new terminals,
`terminal.new_tab_directory` to new tabs and split panes but not new windows,
`terminal.close_on_exit`, `terminal.macos_option_as_alt`, and
`window.macos_fullscreen_mode` to later events, `palette.retain_query` and
`palette.retain_query_seconds` to the next palette open, and every other
setting live. `palette.placement` moves an open palette, so it is live,
unlike the mockup. `quake` applies to later events as a whole: removing a
profile changes its window at once, but a changed profile reaches its window
at the next summon.

### Constraints and descriptions move onto the types

Ranges, patterns, and descriptions that `schema.rs` patches by path move onto
the types as `#[schemars(range(...))]`, `#[schemars(regex(pattern = ...))]`,
`extend`, doc comments, or a type-level `transform`. `schema.rs` keeps only
what no single type owns: the catalog-derived keybinding alternatives, the
selected-theme derivation, null and container-default cleaning, and the
document `$id` and titles.

The regenerated schemas must equal the committed ones once `x-huterm-*` keys
are removed and labelled enums are compared as their value sets. A one-off
comparison script proves it during review; it is not committed.

### Settings model (`huterm_config::settings`)

Behind a new `settings` feature that enables `serde_json`. The model embeds
`schemas/huterm.schema.json` and parses it once.

- `SettingsModel::load()` returns the cached
  `Result<&'static SettingsModel, ModelError>`, and
  `SettingsModel::from_schema(&Value)` builds one from any schema. Missing or
  malformed metadata, an unknown section, apply, or platform value, a
  duplicate order, a blank or misspelled option label, or a visible-when
  naming an unknown setting or a value outside its choices is an error naming
  the setting path.
- `sections()` returns the ten sections in sidebar order with their titles,
  each holding groups ordered by their lowest setting order, each holding
  settings in order. `setting(path)` finds one setting by dotted path.
- A `Setting` holds its path, label, description with each paragraph's
  lines joined, kind, default as a `toml::Value` when the schema has one,
  unit, apply, platform, advanced, visible-when, and whether it is
  deprecated.
- `SettingKind` is `Bool`, `Choice(options)` with each option's value, label,
  and description, `Integer { min, max }`, `Number { min, max,
  exclusive_min }`, `Text { pattern }`, or `Compound` for objects, maps,
  arrays, and type unions. All but `Compound` have a generic control.

### GPUI override table

`huterm-gpui` gains `desktop/settings.rs` with the override table the window
will read: a special control for a setting path. A separate file-only list
names settings Settings never edits; a deprecated setting must be on it. Until
the window lands, the module carries `#[cfg_attr(not(test),
expect(dead_code))]`. Hiding a setting, or moving one behind Advanced, joins
the table when the window needs it.

Initial entries:

- special controls: `theme` (theme picker with color overrides), `keybinding`
  and `global_keybinding` (keybinding editor), `quake` (profile editor),
  `terminal.link_modifiers` (modifier chips), `terminal.scrollback_bytes`
  (size field), and `font.family` (font picker);
- file-only: `themes` and the deprecated `terminal.engine`.

### PR 2 verification

| Behavior | Evidence |
| --- | --- |
| The committed schema parses into a model with ten sections and every setting the config defines | `huterm-config` test with the `settings` feature |
| Every setting has a generic control, an override, or a file-only entry; every entry names a real setting; deprecated settings are file-only | `huterm-gpui` coverage test |
| Every `Choice` option outside overrides and the file-only list has a label | The same coverage test |
| Every value a default configuration writes belongs to some setting, so the walk misses no field | `huterm-config` test serializing a default `RawConfig` |
| Malformed metadata is rejected with the setting path | Focused tests feeding small schemas to `from_schema` |
| Visible-when references resolve and their values are valid choices | Covered by `load()` plus one rejection test |
| Moved constraints keep the schema unchanged | `mise run schema:check` on the regenerated files, the one-off comparison, and `smoke:schema-editor` for Taplo |

`mise run check`, `mise run test`, and `mise run schema:check` gate the PR.
No UX review: nothing a user sees or touches changes.

### Non-goals for PR 2

The Settings window, controls, writes, and inner metadata for themes,
keybindings, and quake profiles.

## PR 3: window and controls

### What PR 3 delivers

`open_settings` opens a Settings window that edits the scalar settings the
settings model lists, saving each change through the patch service. The
window follows the A1 layout: a sidebar with search, the sections, and the
file status in its footer, and a main pane of grouped cards. Keybindings and
Quake windows get their editors in PRs 4 and 5; until then their sections
say so and open the file.

### Window

- A new window kind, `SettingsView`, with no session, workspace, or terminal.
  It is not in `WindowModel`, which tracks terminal windows only.
  `Desktop::settings` holds its handle. `open_settings` focuses it when open
  and opens it otherwise; the file is created from the default template
  first, as today.
- Native decorations on both platforms, titled "Settings", 860 by 620 points
  centred on the active display, at least 640 by 420. Colors derive from the
  active theme through `Swatch`, and a reload repaints them.
- Close Window and the close button close it without confirmation. Closing
  it runs `maybe_exit`, so Huterm exits when it was the last window, as with
  any other window.
- Quit never hosts its confirmation in Settings: `quit_host` already reads
  `WindowModel`, which has no Settings record. Committing Quit closes the
  Settings window with the others. While Settings is the only window, Quit
  exits once the usual assessment finds no sessions.
- `open_settings` becomes an Application command so it works from any
  window, Settings included. Window-scope commands that need a terminal
  window, such as New Tab, are refused in Settings. Close Window, Close Tab,
  and Close Pane close it, and Next Tab and Previous Tab move between its
  sections.
- A new Application command, `open_config_file`, opens `config.toml` in the
  system editor; the footer, the General section, and the configuration
  error notices use it. The existing menu items keep `open_settings`, which
  now opens the window.

### Layout

- Sidebar: a search field, the sections in model order, and the footer. A
  section shows when it has a setting for this platform. Each section shows
  the number of its settings set in the file.
- Main pane: the section title, a banner when needed, then each group as a
  card of rows, in model order. Settings flagged advanced appear only after
  the section's "Show advanced settings" switch is on; search always shows
  them.
- A row shows its label, a dot when the file sets it, its description, an
  apply note when it is not live ("Applies to new terminals"), and an inline
  error. Its control sits on the right with a reset button when the file
  sets it; reset removes the key. The Theme row's Reset removes only
  `theme.name`, keeping color overrides, and the Keybindings, Global
  shortcuts, and Quake rows have none until their editors exist.
- A setting whose visible-when condition does not hold is hidden. A setting
  for another platform is hidden, unless the file sets it, in which case it
  shows read-only with "macOS only. Set in config.toml and ignored here."
- Search filters every section by label, description, and path, and groups
  the results by section.
- General adds a card with the file's path and Open and Show in Finder (or
  Folder) buttons above `watch_config`. Keybindings and Quake windows show
  their rows with an Open config.toml button.

### Controls

Built on `TextField`, `Menu`, `Swatch`, `KeyHint`, and the scrollbars:

| Kind | Control |
| --- | --- |
| `Bool` | Switch |
| `Choice` with up to three short options | Segmented control |
| Other `Choice` | Select, opening a `Menu` of the option labels |
| `Integer`, `Number` | Number field with its unit, checked against its range, stepped by the arrow keys |
| `Text` | Text field, saved on Enter or blur |
| Optional `Bool` (no default) | Select: "Default", "On", "Off" |
| Optional `Integer` | Number field with a "Default" placeholder; clearing it resets |

Special controls from the override table:

- `font.family`: a text field beside a menu of the installed families.
- `theme`: cards for the bundled and adjacent themes, with a note listing
  any `[theme]` color overrides, which apply on top of every theme.
- `terminal.link_modifiers`: modifier chips; macOS offers no Ctrl.
- `terminal.scrollback_bytes`: a text field accepting a size such as
  `64MiB`, empty for no size limit.
- `keybinding`, `global_keybinding`, `quake`: the placeholder cards.

Inputs are checked against the setting's kind before anything is written: a
number outside its range, or text that does not match its pattern, shows the
error under the row and writes nothing. `tabs.min_width` above
`tabs.max_width` is the one cross-setting rule, checked through the patch
service's validator like everything else.

Selects reuse `Menu`: the settings model and the installed font list are
static for the process, so their strings serve as menu item identifiers.

### Saving and status

- The window keeps a base `Snapshot`, loaded and validated on the
  background executor when it opens and after every save and reload. A
  change becomes one `Patch::Set` or `Patch::Remove` and runs
  `ConfigFile::apply` with `config::validator` on the background executor.
  Writes are serialized: a change made while one is in flight queues behind
  it, replacing a queued change to the same setting. A queued change keeps
  the snapshot it was made against, moving on only past this window's own
  writes that met an unchanged file, so it conflicts with an edit made
  elsewhere instead of overwriting it.
- Controls show a change's value until its write settles, so repeated steps
  and toggles build on each other. Closing the window saves a focused
  field's draft and waits for pending writes; Quit does not wait.
- After a successful write the window asks for a manual reload, so the
  change applies whether or not watching is on, and shows "Saved".
- The footer shows one status: "Watching config.toml", "Not watching
  config.toml", "Saved", "Reloaded", "config.toml changed on disk" with
  Reload, or an error with its action. While watching is off, the window
  checks the file when it comes forward.
- An invalid file, from the window's own validation or from
  `ApplyError::InvalidFile`, puts a banner over the controls with the
  parser's message and an Open config.toml button, plus Reload while
  watching is off, and makes every control read-only until the file
  validates. Pending changes settle unsaved. The running settings stay as
  they were. A reload that fails for a reason outside the file, such as a
  global shortcut another application holds, shows "Not applied" instead.
- `ApplyError::Conflict` keeps the entered value in its control, marks the
  row, and shows the conflict with "Use mine", which reapplies the value on
  a freshly loaded base, and "Use file", which discards it. Other write
  errors show the message in the footer and under the row; a field keeps
  its text so it can be corrected, and other controls return to the file's
  value.
- A reload refreshes the base and every control. A field holding an unsaved
  draft keeps it, and the row shows a conflict when the file changed that
  setting. Escape in a field puts back its value.

### Keyboard

Every action is a catalog command with Window scope, following the overlay
grammar. `settings_search`, `settings_focus_next`, and
`settings_focus_previous` bind under `Settings`. `settings_toggle`,
`settings_increase`, `settings_decrease`, and `settings_reset` bind under
`SettingsControl`, which only switches, selects, segmented controls, and
section rows carry, so Space and the arrows reach text fields. Text fields
take the `Palette` context for editing, as the Licenses view does: Enter
saves a field or moves from the search to the first match, Up and Down step
a number, and Escape clears the search. Tab and Shift-Tab move through GPUI
tab stops.

### PR 3 files

- `crates/huterm-gpui/src/desktop/settings/`: `values.rs` (pure value
  lookup, defaults, input checks, and patch building) and `status.rs` (the
  footer state).
- `crates/huterm-gpui/src/desktop/windows/settings_window.rs` (the view and
  its commands) with `controls.rs`, `render.rs`, and `saving.rs` (loads,
  the write queue, conflicts, and drafts).
- `windows.rs`: opening and focusing the window, reload notifications, and
  Quit.
- `huterm-protocol` and `keymap.rs`: the new commands and their bindings.
- `docs/configuration.md`, `docs/keybindings.md`, `README.md`, and
  `docs/agents/desktop-client.md`.

### PR 3 verification

| Behavior | Evidence |
| --- | --- |
| Value lookup, defaults, input checks, and patch building for every generic kind | `values.rs` unit tests |
| Footer status transitions: save, reload, external change with watching off, invalid file, conflict | `status.rs` unit tests |
| Opening focuses an existing window; closing exits when it was the last; Quit closes it | GPUI tests through the desktop harness |
| A switch, select, number field, and reset each write only their key, preserving comments | GPUI tests writing a fixture file and reading it back |
| A change applies with watching off | GPUI test |
| An invalid file makes controls read-only; a conflict keeps the entered value | GPUI tests |
| Keyboard: open Settings, search, move between controls, toggle, step, reset, and return to a terminal | A `settings` step in `smoke:macos-palette` and `smoke:linux-palette` (`HUTERM_PALETTE_SMOKE_ONLY=settings` runs it alone), and the `macos-settings` and `linux-settings` CI smoke steps |
| The window in the running app | `ux-review` |

### Non-goals for PR 3

The keybinding editor, the quake profile editor, global shortcuts, editing
theme colors, and themes defined in `[themes]`.

## PR 4: keybinding editor

### What PR 4 delivers

The Keybindings section lists every catalog command with its shortcuts and
edits `[[keybinding]]` entries in place, following the revised A1 mockup
("Expanding command rows"). A command row expands to show each of its
shortcuts with a key recorder, a `when` field with completion, argument
controls, a Default or Custom tag, an enable switch, and, for the user's own
entries, a remove button. Global shortcuts and Quake profiles stay
placeholders until PR 5.

### Keybinding list

- `keymap.rs` gains an inventory beside `compile`: the same resolution over
  the platform defaults and the file's entries, recording what happened to
  each binding instead of only the survivors. A default is active, disabled
  by a `disabled = true` entry, replaced by a later entry with the same keys
  and combined predicate, or removed by an `unbind`. A user entry is active,
  switched off by its own `disabled = true`, replaced, or unbound, and keeps
  its index for an `EntryLocator`. The Settings window builds it from the
  file it read, not the installed keymap, so it is right with watching off.
- Commands appear in groups: Tabs, Panes, Window, Terminal, Application,
  Quake, and "Overlays and text fields" for commands with an implied context
  (`Palette`, `menu`, `notices`, `confirming`, `Settings`,
  `SettingsControl`). A GPUI-side table assigns the groups, and a test fails
  when a catalog command has none.
- A collapsed row shows the title, description, a dot when the command is
  customized (it has a user entry, or a default is not active), up to two
  active shortcuts as key caps, "+N" for more, a "when" chip when an active
  shortcut has a condition, and "No shortcut" when none is active.
- The section has its own search field, matching titles, command ids, key
  text, and conditions, and a segmented filter: All, Customized,
  Conditional, and Disabled. The sidebar search also lists matching
  commands under Keybindings; choosing one opens the section with that
  command expanded.
- `unbind` entries appear after the groups as "Passed through to programs",
  each with a remove button.
- The sidebar count for Keybindings is the number of `[[keybinding]]`
  entries in the file, matching the other sections' count of settings the
  file sets.

### Keybinding edits

Each edit is one `ConfigFile::apply` of one or more patches, so a pair of
entries lands together or not at all. Patches address user entries through
`EntryLocator`s from the window's snapshot.

| Edit | Patches |
| --- | --- |
| Switch off an active default | Append `{key, command, when, args, disabled = true}` matching exactly that default (`when` and `args` only when the default has them) |
| Switch on a disabled default | Remove the entries that disable it |
| Switch a user entry off or on | Set or clear its `disabled` field |
| Change a default's key, `when`, or argument | Append the disabling entry (unless one exists) and a replacement entry with the changed binding |
| Change a user entry's key, `when`, or argument | Set or clear that field |
| Add shortcut | Record a key, then append `{key, command}` |
| Remove a user entry | Remove it |
| Restore defaults (per command) | Remove every user entry for that command; `unbind` entries stay |
| Remove an `unbind` entry | Remove it |

Settings never writes `unbind`. Switching a user entry off writes
`disabled = true` on it, which also turns off an earlier binding with the
same key, command, and condition; such an entry duplicates that binding, so
the effect is the same.

Keybinding changes join the window's write queue behind any other write.
Each edit's patches are built when it is made, from locators the
background read made for the snapshot the list came from; a locator finds
its entry by position and content, so a change made elsewhere to that
entry makes the edit conflict instead of editing another shortcut. When
the window's own write lands, queued locators for the entries it changed,
matched by position and content, take their new content, those for entries
it removed go, and later positions move up, so successive edits to one
entry build on each other. A default with a change queued or being
written takes no other change until it lands, since the list still shows
it active; focus then follows to the new row. A read that finishes while
the window's writes are pending is dropped, so queued edits and the list
never mix snapshots, and the file is read again once the queue drains. The
validator compiles the keymap, so a binding the compiler refuses is never
written. A field whose write fails, including against a file that turned
invalid, keeps the entered text with the error to retry.

Fields follow their shortcut across reads: when another editor shifts the
entries, a field and its draft move to the row its binding now has; a
binding changed in place that keeps its key and command keeps its fields;
any other change drops them.

### Key recorder

- Clicking a key cap, Space or Return on it, or Add shortcut starts
  recording; the cap reads "Press keys", and a line under the row says how
  recording ends. While it records, an
  application keystroke interceptor takes the window's key-down events and
  stops their propagation, so no binding, menu key equivalent, or text field
  acts on them. A key that the operating system or a registered global
  shortcut takes first cannot be recorded.
- Modifier-only presses are ignored. Escape cancels at any point; Return
  after a stroke saves at once. A second stroke within one second, GPUI's
  own chord timeout, records a two-stroke chord; recording ends after the
  second stroke or one second after the first. Focus leaving the cap, the
  window losing focus, or a click elsewhere cancels. The cap shows the
  strokes so far as key glyphs.
- The result is written in the form `docs/keybindings.md` documents and
  `parse_keystrokes` accepts, including shifted punctuation as its symbol.
  Recording the binding's current key writes nothing.

### Keybinding conditions

- Each shortcut's `when` is a text field (placeholder "Anywhere", or the
  implied context for commands that have one) saved on Enter or blur, like
  the scalar fields. Escape puts back the saved condition and keeps focus.
  Only an active shortcut's fields edit; a field keeps a draft only while
  it is being edited.
- Typing a context name offers completion from a code-level list of
  contexts with their descriptions, the list `docs/keybindings.md`
  documents; a test fails when the two differ. An empty name offers
  nothing. Up and Down move through the suggestions and Return takes one;
  Tab and Escape first close an open list, keeping the text, and act
  normally after that.
- GPUI's predicate parser checks the text before anything is written: a
  syntax error shows under the shortcut and writes nothing. A known-syntax
  condition naming an unknown context saves with a warning that it never
  matches.

### Keybinding arguments

Only the arguments a config can carry appear, on a line under the
shortcut, each labeled before its control; tab, pane, workspace, and
session targets never do. Up and Down step a number field. Every argument
is optional in a binding: an empty control writes nothing, and the palette
prompts for a prompted argument or the command uses its default.

| Kind | Control |
| --- | --- |
| `Bool` | Select: "Default", "On", "Off" |
| `Integer` | Number field with the range from the catalog, empty for none |
| `Text` | Text field, saved on Enter or blur |
| `QuakeProfile` | Select of the configured profiles; unset shows `default` |
| `Side` | Select of the sides, plus "Ask" |

### Keybinding messages

Under each shortcut: an error for the typed text, then notes about the
file's binding from one pure function, `notes`:

- what became of it: switched off, replaced (naming the shortcut and
  command that took the key), or passed through by an `unbind` entry;
- a warning on the shortcut that took a key from another command;
- for a user shortcut sharing its key with another command, where the other
  uses it, with a warning when the shortcut has no condition and so can
  take the key there;
- a warning on a user shortcut without a condition on a bare key, and a
  note on Ctrl and a letter, which terminal programs stop receiving;
- a warning for a saved condition naming an unknown context.

Pairs of shipped defaults get no notes. Restore defaults shows only when
the command has entries to remove, and says when they include shortcuts the
user added.

### Keybinding saves and status

Saving follows PR 3: "Saved" and a manual reload after each write, an
invalid file makes every control read-only, and other failures show in the
footer. A conflict on a keybinding write, such as an entry another editor
changed, shows "Not saved" with the patch service's message and refreshes
the list from the file; a binding edit can span several fields and entries,
so it has no Use mine or Use file.

### Keybinding keyboard access

Command rows are `SettingsControl` stops: Space and Return expand and
collapse them, and `settings_reset` restores the command's defaults. Inside
an expanded row, Tab reaches the key cap, the `when` field (a `Palette`
text field), argument controls, the switch, the remove button, Add
shortcut, and Restore defaults. In the section, `settings_search` focuses
its own search, and Return there moves to the first command listed. No new
commands or bindings.

Focus never stays on a control that went away: removing a shortcut or
restoring defaults moves it to the command row, removing an `unbind` entry
to the section's search, and a changed default's new row takes focus on
the control that changed it. Anything else falls back to the window, so
the keyboard keeps working.

### PR 4 files

- `crates/huterm-gpui/src/keymap.rs`: the inventory.
- `crates/huterm-gpui/src/desktop/settings/keybindings.rs`: command groups,
  the context list, condition checks, and turning edits into patches.
- `crates/huterm-gpui/src/desktop/windows/settings_window/keybindings.rs`:
  the section, its rows and notes, the recorder, and focus recovery; the
  interceptor is registered in `settings_window.rs`.
- `docs/keybindings.md`, `README.md`, and this plan.

### PR 4 verification

| Behavior | Evidence |
| --- | --- |
| Inventory states for defaults and user entries: active, disabled, replaced, unbound, switched off | `keymap.rs` unit tests |
| Each edit's patches, applied to a fixture file, keep comments and other entries and compile | `keybindings.rs` unit tests |
| Condition checks and completion; the context list matches the docs table; every command has a group | `keybindings.rs` unit tests |
| Switching a default off and on, changing a default's key, editing and removing a user entry, adding a shortcut, and restoring defaults write the expected entries | GPUI tests through the desktop harness |
| The recorder takes keys bound elsewhere (`cmd-w` does not close Settings), records a chord, and cancels on Escape | GPUI tests with simulated keystrokes and the test clock |
| An invalid condition writes nothing; an invalid file makes the editor read-only | GPUI tests |
| Recording a shortcut and switching a default off with native keys, checking the file and that the switched-off key no longer runs its command | The `settings` step of `smoke:macos-palette` and `smoke:linux-palette` |
| The editor in the running app | `ux-review` |

### Non-goals for PR 4

Global shortcuts and Quake profiles (PR 5); editing an entry's
`description`; writing or editing `unbind` entries beyond removing them;
reordering entries; and showing other platforms' defaults.

## PR 5: quake profiles and global shortcuts

### What PR 5 delivers

The Quake windows section edits `[quake.profiles.*]` tables and
`[[global_keybinding]]` entries in place, replacing its two "Open
config.toml" rows. A profile dropdown picks the profile to edit, New profile
adds one, and the profile's card shows a control for each profile setting.
A Global shortcuts card under it lists every global shortcut with a key
recorder, its command, its profile, and a remove button. This is the last
PR of the series; it closes #35.

### Profile settings

- Profile fields gain `x-huterm-*` metadata on `huterm_config::quake::Profile`,
  as PR 2 did for other settings: label, order, unit, apply, and
  visible-when. Every field applies at later events: the next summon reads
  the profile. `width` and `height` show only while `fullscreen` is off.
- `SettingsModel` walks the `Profile` definition with the same rules into a
  template of settings, and Settings makes one copy per profile with paths
  under `quake.profiles.<name>`. A test fails when a `Profile` field has no
  setting. The copies are interned, as PR 4 interns profile names, since
  controls hold `&'static Setting`.
- The fields use the generic controls: a select for `position` and
  `animation`, switches for `fullscreen` and `hide_on_focus_loss`, number
  fields for `width` and `height` (fractions of the display's work area,
  greater than 0 through 1, as the file stores them; the mockup's percent
  sliders would need a control the kit lacks) and `animation_ms` (0 through
  1000 ms). A dot
  marks a value the file sets, and Reset removes it, as for other settings.
- `display` is a select of Active display, Display under the pointer, and
  Primary display. A file value `id:<identifier>` shows as one more option
  labeled with the identifier; listing connected displays is a non-goal.
- Writes join the window's setting queue and conflict checks unchanged;
  their paths are dotted, so profile names Settings edits are bare TOML keys
  (letters, digits, `_`, and `-`). A profile whose name needs quoting is
  listed, and its card says it stays in `config.toml`; dotted paths cannot
  reach its values.

### Profiles

- The dropdown lists `default` first, then the file's profiles by name. It
  starts on `default` and keeps its choice while the window is open; a
  profile that disappears on reload falls back to `default`.
- New profile opens an inline name field with Create and Cancel. Return
  creates, Escape cancels. A blank name, a name that needs quoting, or an
  existing name shows an error under the field and writes nothing. Create
  writes an empty `[quake.profiles.<name>]` table and selects the profile.
- A named profile's card ends with Delete profile. It asks inline, as in "Delete
  profile logs? Its settings and comments are removed from config.toml.",
  with Delete and Cancel, and focus on Cancel. Deleting removes the table,
  and the dropdown returns to `default`. When a global shortcut uses the
  profile, the delete is refused with a message naming the shortcut's key,
  because the validator would reject the file. The note under the button
  says that an open window of a deleted profile becomes an ordinary window
  and keeps its tabs.
- `default` has no Delete. Its fields reset like any profile's; removing its
  last value leaves an empty table, which the patch service keeps as written.

### Global shortcuts

- The card lists `[[global_keybinding]]` entries in file order. Each row has
  a key cap, a command select (Toggle Quake, Show Quake, Hide Quake), a
  profile select (configured profiles; unset shows `default`), and a remove
  button. Add global shortcut records a key and appends `{key, command =
  "toggle_quake"}`, with `args = { profile = <name> }` when the selected
  profile is not `default`.
- The key cap uses PR 4's recorder in a single-stroke mode: recording ends
  after the first non-modifier stroke, and Return and Escape keep their
  meanings. Recording the current key writes nothing.
- Edits address entries through `EntryLocator`s for `List::GlobalKeybinding`,
  parsed off the GPUI thread with the keybinding entries, and join the
  window's keybinding queue with the same rebasing and read rules.
- The validator already compiles global shortcuts, so a key that conflicts
  with the first stroke of an in-app binding, a duplicate key, or an
  unsupported key shows its message under the row and writes nothing. A
  registration the operating system refuses shows after the reload, in the
  footer, as PR 3 reports reload errors.
- Under a Wayland or XWayland session the card says global shortcuts need
  macOS or a native X11 session, and still edits the file.
- An empty card says no global shortcuts are set and offers Add global
  shortcut.

### PR 5 keyboard access and status

Every Settings menu opens on the value its control shows, and only a moving
pointer selects a row, so Return right after opening keeps the value. The
New profile field takes the `Palette` context like other Settings fields,
and Escape closes the delete confirmation through the window's keystroke
interceptor, so no binding is added.

The dropdown, New profile, the name field and its buttons, the fields,
Delete profile, and every global shortcut control are tab stops in that
order. Removing a global shortcut moves focus to Add global shortcut;
deleting a profile moves focus to the dropdown. In a sidebar search, the
Quake profiles and Global shortcuts rows offer Show, which opens the
section.
Saving, conflicts, invalid files, and read-only behavior follow PR 3. The
sidebar count for Quake windows is the number of profile values the file
sets plus the number of global shortcuts. No new commands or bindings.

Creating and deleting a profile have no row to hold a conflict, so a refused
one settles at once: the profile bar shows the message, a refused creation
reopens the name field with its name, and the file is read again. Delete
reads the file afresh, since the user confirmed deleting the profile as it
is. A global shortcut needs Ctrl, Alt, or Cmd (Super on Linux), or a
function key, because it reaches every application; Settings refuses
others, while the file still accepts them. An open global shortcut menu or
recorder closes when its own shortcut changes, because it names the
shortcut by position. Picking the value the file already holds, or will
hold once a pending change lands, writes nothing. A refused creation keeps
a name typed since for another profile, and drops edits queued for the
refused profile with their field drafts; a creation still queued when the
file turns invalid gives its name back.

### PR 5 files

- `crates/huterm-config/src/quake.rs` and `settings.rs`: profile metadata
  and the template.
- `crates/huterm-config/src/document/splice.rs`: `Set` of a table of plain
  values that does not exist writes its own `[section]`, and `Remove` of a
  table written as sections removes them with their comments. The profile
  map joins the tables that go once empty.
- `crates/huterm-gpui/src/desktop/windows/settings_window/quake.rs`: the
  section, the profile dropdown and lifecycle, and global shortcut rows.
- `crates/huterm-gpui/src/desktop/settings/`: global shortcut edits as
  patches, beside the keybinding edits.
- `docs/configuration.md`, `README.md`, and this plan.

### PR 5 verification

| Behavior | Evidence |
| --- | --- |
| Every `Profile` field has a template setting with its range and default | `settings.rs` unit tests |
| Changing and resetting a named profile's field writes only that key | GPUI tests through the desktop harness |
| New profile writes an empty table; blank, quoted, and duplicate names write nothing | GPUI tests |
| Deleting a profile removes its table; a delete refused while a global shortcut uses it | GPUI tests |
| Recording, changing the command and profile of, and removing a global shortcut write the expected entry | GPUI tests and edit unit tests |
| A global key that conflicts with an in-app binding shows the validator's message and writes nothing | GPUI test |
| A quoted profile name shows read-only | GPUI test |
| Changing the default profile's height and recording a global shortcut with native keys, checking the file | A `settings` step in `smoke:macos-palette` and `smoke:linux-palette` |
| The section in the running app | `ux-review` |

### Non-goals for PR 5

Renaming profiles; listing connected displays for `display`; editing
`args` other than `profile`; and checking a key against other
applications' shortcuts before the reload.

## Later PRs

Each later PR gets its own contract when it starts, built on this section.

1. **Settings model and metadata.** Its contract is PR 2 above. Semantic
   metadata lives on the config
   types and is emitted as `x-huterm-*` schema extensions through
   `#[schemars(extend(...))]`: label, group, order, unit, apply semantics
   (live, new tabs, new terminals, later events, next palette open),
   platform, advanced, and visible-when. Constraints and descriptions now
   patched in `schema.rs` move onto the types. A typed `SettingsModel` parsed
   from the committed schema drives the UI. A test fails when a schema leaf
   has neither a generic control, an override, nor a place on an explicit
   file-only list.
2. **Window and controls.** Its contract is PR 3 above. The Settings
   window kind and its Close and Quit
   handling, the A1 shell, and the sidebar footer status. A control kit built
   on existing pieces (`ui/text_field.rs`, `MenuModel`, `Swatch`, `KeyHint`,
   and scrollbars): switch, segmented control, select, number field with unit
   and range, modifier chips, and form row. It also covers the scalar
   settings, reset, validation, and the theme picker with color overrides. A
   GPUI-side override table gives special controls, hides settings, or puts
   them behind Advanced.
3. **Keybinding editor.** Its contract is PR 4 above. Expanding command
   rows, a key recorder with a
   capture context the window keymap cannot intercept, `when` completion from
   the context list and validation, argument controls, and the enable switch
   that writes `disabled` entries.
4. **Quake profiles and global shortcuts.** Its contract is PR 5 above.
   Quake profiles with a profile dropdown, and global shortcuts.

Native keyboard and focus smokes cover navigation, save, reset, and returning
to a terminal, as issue #35 requires.

## Open questions

None for PR 1.

[issue-14]: https://github.com/jimeh/huterm/issues/14
[issue-34]: https://github.com/jimeh/huterm/issues/34
[issue-35]: https://github.com/jimeh/huterm/issues/35
[mockups]: https://plans.jimeh.dev/ebw3yvexaw6v76vs24krpasaoe/huterm-settings-round2.html
[mockup-a1]: https://plans.jimeh.dev/2pjfyytgq3lqoe5c5iduqjuzlm/huterm-settings-a1-revised.html
