# Session restore (#42)

Status: implemented. Pull request #404 delivered core and persistence; the
desktop pull request delivered restore. Jim accepted every recommendation on
2026-11-04.

Delivers [#42](https://github.com/jimeh/huterm/issues/42) under the
[#17](https://github.com/jimeh/huterm/issues/17) epic. Styled history is
[#43](https://github.com/jimeh/huterm/issues/43) and stays out.

## Outcome

Quitting Huterm and starting it again brings back every surviving session
with its workspaces, tabs, split panes, names, and windows. Each pane runs a
fresh shell in the directory it last reported. A crash or a killed process
restores the last checkpoint instead. Closing the last window is not a Quit:
it ends that session, so the next launch starts fresh.

Restore recreates structure, never processes. Nothing saved is executed or
interpreted as a command.

## Current state

- Core: `Mux::capture_hierarchy` returns `HierarchySnapshot` (sessions,
  workspaces with tab layouts, attachments), versionless and in memory. Every
  scoped ID (session, workspace, tab, pane, split, attachment) and
  `TerminalId` comes from one per-runtime counter (`next_id`);
  `reserve_through` raises it. Automatic names come from `session_ordinal`
  and `workspace_ordinal`. `HierarchyState` (protocol) has neither the
  counter nor the ordinals.
- `commit_close_with` (`mux/lifecycle.rs`) calls its capture callback after
  every rejection path and before teardown; `Mux::shutdown` then clears the
  structure and emits `Reset`.
- Core does not keep a terminal's launch directory. `TerminalMetadata` has
  the effective directory and the root shell's last local report, readable
  from each terminal's registry status without blocking its thread.
- `TerminalRuntime::spawn_in` blocks until the PTY and child start, so spawns
  must stay off the UI thread.
- GPUI: `DesktopRuntime::capture` keeps the first `RestoreSnapshot {
  hierarchy, windows }` (`get_or_insert_with`) from the Quit dialog's commit,
  native termination (`on_app_quit`), or the post-loop `terminate()`. Nothing
  reads it. `WindowRestore` holds attachment, workspace, active tab, bounds,
  tab position, sidebar width, and Quake profile. Its bounds are the
  restorable bounds, which stay windowed through fullscreen
  (`fullscreen.rs`), so fullscreen itself is not recorded.
- Startup subscribes the projection to the empty Mux before
  `application.run`, installs Quake shortcuts, and opens one fresh window;
  `on_reopen` can open another. `maybe_exit` quits once no window, pending
  spawn, session, or Quake keepalive remains.
- Tab tear-off creates an empty session and workspace before its window
  opens and the tab moves in (`cross_window.rs`). Its orphan cleanup reads
  `HierarchySnapshot.attachments`.
- Quake `attach` retargets the new window to visible at once
  (`quake_windows.rs`), so it cannot create a hidden window as is.
- Pane focus per tab lives in the view. Layout history holds runtime-scoped
  records and sequence numbers.
- No session switcher exists. Each desktop window owns one private session
  with one workspace, so an unviewed session cannot arise from the UI today,
  but the runtime allows it.

## Decisions

### On-disk scope and files

```text
$XDG_STATE_HOME/huterm/            (default ~/.local/state/huterm/, both platforms)
  sockets/<socket-name>/
    lock                          flock target, held for the process lifetime
    runtime.json                  runtime structure for the socket scope
    desktop.json                  the desktop client's layout
    restore-attempt               crash-loop marker, present only during restore
```

- `HUTERM_STATE_DIR` overrides the root, as `HUTERM_CONFIG_FILE` does for
  config.
- XDG on macOS too, matching the config path (`~/.config/huterm/`). The IPC
  plan (#44, PR #386) puts sockets under `$XDG_RUNTIME_DIR/huterm/` or
  `/tmp/huterm-<uid>/`; both clear at logout or reboot, so state never lives
  beside the socket.
- Directories are created `0700`, files `0600`; saved paths and names are
  private.
- Socket names use the IPC plan's grammar, `[A-Za-z0-9][A-Za-z0-9._-]{0,63}`,
  so the same name works as a path component and a socket. Any other name
  disables persistence for that scope. Embedded mode uses only `default`.
- Runtime structure and client layout are separate files, because in server
  mode the daemon owns `runtime.json` and each client kind owns its layout.

### Identity

Three identities stay distinct:

- `scope_id`: 16 random bytes, generated when `runtime.json` is first
  written and kept across restores. It is the IPC plan's stable server ID.
  A replaced or discarded `runtime.json` gets a new one.
- `ServerIncarnation` (IPC plan): random per cold start, for server mode. A
  restore from saved state is a cold start. Embedded mode has none.
- `RuntimeId`: process-local scope for in-memory IDs, as today.

Restore keeps the saved numeric IDs and gives them the new `RuntimeId`. A
saved client layout may name those numeric IDs only when its `scope_id` and
`generation` match the runtime file's (see "Two files").

### File format and versioning

- JSON through `serde` and `serde_json`, both already pinned. Each file has
  `"format"` (`"huterm-runtime"` or `"huterm-desktop-layout"`), an integer
  `"version"` (1), `"scope_id"`, and `"generation"`.
- Core and protocol stay serialization-free. Protocol gains plain std-only
  capture types (sketch: `RuntimeCapture` with the allocator watermark, both
  ordinals, ordered sessions, workspaces, tabs with `PaneLayout`, and per
  terminal its directory candidates and launch tag). Core produces it and
  restores from it. A new `huterm-state` crate holds the serde DTOs, the
  conversions to and from the protocol types, paths, locking, and writes. It
  depends on `huterm-protocol`, `serde`, `serde_json`, and `nix` only, and
  `mise run architecture` checks that. GPUI depends on it; core does not.
- Additive optional fields do not bump the version, and readers ignore
  unknown fields; an older build that rewrites the file drops them, which is
  acceptable for optional data. A change in meaning bumps the version and
  adds a migration from each older version, run in memory at load. Version 1
  has nothing to migrate; the hook is the version gate plus this rule.
- A newer version is not restored and not overwritten: that run keeps
  persistence off for the scope and says so.
- Bounds, the same for writer and reader: file size 4 MiB; 256 sessions,
  1,024 workspaces, 4,096 tabs, 16,384 panes, layout depth 32; the runtime's
  name rules; paths 4,096 bytes. The reader checks the file size before
  reading, stops each list at its bound while parsing, and checks aggregate
  counts, path lengths, and depth after parsing; the 4 MiB cap bounds
  memory. A capture over a bound is not written, and the run reports it
  once. Directory candidates are optional, so the writer drops one that is
  not UTF-8, not absolute, or over the path bound instead.
- Validation is all-or-nothing: a duplicate ID across kinds, an ID above the
  saved watermark, an orphaned reference, or an invalid layout rejects the
  file.

### Runtime record (`runtime.json`)

- Allocator watermark (`next_id`) and both creation ordinals.
- Sessions in order: ID, custom and automatic names, workspace order.
- Workspaces: ID, custom and automatic names, tab order.
- Tabs: ID, custom name, pane layout (split IDs, axes, and weights; pane IDs,
  terminal IDs, and custom names), shared zoom, input sync.
- Per terminal: up to three directory candidates in order (effective local
  directory, root shell directory, launch directory) and a `launch` tag.

`launch` is `"default_shell"` in version 1: restore runs the currently
configured shell. No program, argument, environment, title, process name,
command record, or exit state is saved. A pane whose shell had exited at
Quit restores with a fresh shell. A future launch profile adds a tag value,
which this reader treats as a newer-version feature.

Capture drops sessions that hold no tab. Only transfers create them in
flight, such as a tear-off before its tab moves in, so restore never
resurrects one.

Attachments, viewers, and layout history are never saved; a restored
window's undo history starts empty. `HierarchySnapshot` and
`RestoreSnapshot` go away. The tear-off orphan cleanup uses a narrow
`Mux::session_attached(session) -> bool` query instead.

### Client layout (`desktop.json`)

Windows in opening order, each with:

- session and workspace IDs, active tab, tab activation history, and the
  focused pane of each tab (moved from the view into `WindowRecord`);
- restorable bounds relative to the window's display, that display's UUID
  (`display`, optional, added without a version bump), and the current
  mode: windowed, maximized, or fullscreen (new capture data);
- sidebar width; tab position is global config and not saved;
- Quake profile name, if any;
- `appearance`: optional, for #399's per-window transparency overrides. If
  #399 lands first, this work saves them; otherwise #399 adds the field
  without a version bump.

Plus the regular window that was active at capture, which is activated last.

### Two files

Every save writes both files with the same `scope_id` and a `generation`
that increases by one per save: `runtime.json` first, then `desktop.json`. At
load, a `desktop.json` whose `scope_id` or `generation` differs from the
runtime file's is ignored: every restored session opens in its own window at
the default size, with an info notice. That covers a crash between the two
renames and a layout left from a discarded runtime file, where numeric IDs
could now name different objects. In server mode the daemon writes the
runtime generation and a client binds its layout to `scope_id` only,
validating IDs against the live hierarchy.

### Locking and a second process

- At startup the desktop takes `flock(LOCK_EX | LOCK_NB)` on the scope's
  `lock` file and holds it until exit. The kernel releases it when the
  process dies, so no stale lock needs cleaning.
- A process that cannot take the lock runs isolated: it neither
  restores nor writes, and its first window says so. Matching socket names
  never attach to another process's sessions in embedded mode.
- A future daemon (#45) takes the same lock, so an embedded desktop and a
  server for one name exclude each other. A server-mode GUI client takes
  `desktop.lock` for its layout file instead.

### Atomic writes

Write `<name>.tmp` in the same directory, `fsync` it, `rename` over the
target, then `fsync` the directory. An interrupted write leaves the old file.
Only the lock holder writes, so it removes leftover `.tmp` files at startup.

### The persistence owner

One GPUI-owned `Persistence` object orders every write. It runs a single
writer thread and has one state:

| State | Meaning |
| --- | --- |
| `Off` | Setting off, lock lost, newer version, or bad socket name. Rejects writes. |
| `Bootstrapping` | Restore in progress. Rejects writes. |
| `Running` | Accepts checkpoints. |
| `Final` | Took the Quit write. Rejects everything after it. |

- **Checkpoints**: GPUI marks state dirty on a committed
  hierarchy change, a window layout change, and a terminal directory change,
  and submits a capture after 1 s without further changes. A capture taken
  while a cross-window transfer is reserved waits until it settles. The
  writer skips a capture whose bytes match the last write. Nothing runs
  while nothing changes, so idle wakeups do not grow.
- **Final write**: every application exit funnels through the existing first
  capture: the Quit dialog's commit callback (after validation, before
  teardown), native termination, the post-loop `terminate()`, and
  `maybe_exit`'s quit after the last window closed. The first capture moves
  `Persistence` to `Final`, cancels the pending debounce, waits for any
  write in flight, and writes synchronously. Later captures, including the
  empty hierarchy left after `shutdown`, are rejected. After the last window
  closes, that capture holds no session, so the files record an empty set
  and the next launch starts fresh. The PR measures the final write's
  duration on both platforms.
- **Setting off**: moves to `Off`, cancels the debounce, waits for the write
  in flight, then deletes the scope's files. Files with a newer version are
  left alone. **Setting on** moves to `Running` and submits a capture.

### Startup and restore

1. Before `application.run`: resolve the scope directory, take the lock,
   remove `.tmp` leftovers, read and validate both files (at most 4 MiB
   each). A leftover `restore-attempt` means the previous restore never
   finished: rename both files to `.corrupt`, skip restore, and report it.
2. With a restorable record, the launch callback installs the desktop in a
   bootstrap state instead of opening a fresh window. Reopen, Quake
   shortcuts, checkpoints, and `maybe_exit` wait until bootstrap settles.
3. A background task writes and syncs `restore-attempt`, then spawns each
   terminal with `TerminalRuntime::spawn_in` outside the Mux lock. GPUI
   supplies each command: current shell settings, the configured initial
   grid and resolved cell size, and the first directory candidate that
   `usable_launch_directory` accepts, else the default directory. A start
   outside the first candidate records a fallback. The task checks the
   terminating flag between spawns.
4. Under one short lock, `Mux::restore(capture, spawned)` checks that the Mux
   is still fresh (empty, with no identity or ordinal allocated) and has the
   capture's socket name, sets the allocator to the watermark, restores the
   ordinals, and inserts sessions, workspaces, and tabs with their saved IDs
   and the spawned runtimes. A pane that failed to spawn leaves its layout,
   and a tab with no pane left is removed. It returns a report.
5. The UI thread installs the populated hierarchy snapshot and subscription
   through `install_hierarchy`, then opens the restored windows, each
   attached to its session, on its saved display when a display with that
   UUID is connected, at the same position clamped onto its visible area;
   otherwise on the primary display, clamped onto it and recentred when the
   saved area meets none. Each window subscribes its restored terminals
   with `spawned: true`, so a shell that exited before its view existed
   still reports exit and honours `close_on_exit`.
6. A session with no tabs left gets one fresh tab. A session with no saved
   window, including an unviewed one, opens in a new regular window, because
   no switcher exists to reach it.
7. A Quake profile's session is not shown at launch. The registry keeps it
   for its profile, and the first summon opens the Quake window attached to
   it instead of spawning. If the profile is gone or its shortcut failed to
   register, the session opens in a regular window.
8. When every window has opened or failed, bootstrap settles: remove
   `restore-attempt`, move `Persistence` to `Running`, release the gates,
   submit one checkpoint, and activate the saved active regular window.

Quit during bootstrap: the task stops before its next spawn and closes what
it spawned. `Persistence` never reached `Running`, so nothing is written; the
quit removes only the marker, and the next launch restores the same state.

Restore spawns every pane's shell at launch, hidden tabs included. Lazy
spawning would need a pending-terminal state in core and is a non-goal.

### Failures

| Failure | Files | Persistence this run | User sees |
| --- | --- | --- | --- |
| State directory or lock file cannot be created | Untouched | Off | Persistent notice, first window |
| Lock held by another process | Untouched | Off | Persistent info notice, first window |
| `runtime.json` corrupt, over bounds, or invalid | Renamed `.corrupt` (one kept); `desktop.json` ignored | Running, fresh `scope_id` | Fresh window; persistent notice |
| `runtime.json` newer version | Untouched | Off | Fresh window; persistent notice |
| `runtime.json` unreadable (I/O error) | Untouched | Off | Fresh window; persistent notice |
| `desktop.json` corrupt, newer, unreadable, or mismatched | Corrupt one renamed; a newer or unreadable one is left untouched and never overwritten or set aside, while runtime saves continue | Running | Default windows per session; info notice |
| Rename aside fails | Untouched | Off | Persistent notice |
| Leftover `restore-attempt` | Both renamed `.corrupt`, except a newer or unreadable file, which stays in place | Running, fresh state | Fresh window; persistent notice |
| A pane fails to spawn | Next checkpoint drops it | Running | Persistent warning in its window |
| Every pane of a session fails | Session keeps one fresh tab | Running | Warning in its window |
| Fresh tab for an emptied session fails | Session closed | Running | Warning in the next window |
| A restored window fails to open | Its session closed, so nothing unreachable keeps running | Running | Warning in the next window |
| A write fails (disk full, `fsync`, rename) | Previous files kept | Running; retries at next change | One persistent notice per run |
| Quit during restore | Unchanged; marker removed | Not started | Nothing |

### Setting

`[general] restore_sessions = true` (default on), shown in Settings > General
as "Restore windows and sessions at launch". Off: no restore, no writes, and
the lock holder deletes the scope's files (not newer-version ones) at
startup or when the setting turns off during a run.

### Notices

Silent when everything restores as saved. Otherwise one notice per affected
window, through the existing notice stack:

| Case | Severity | Copy (draft) |
| --- | --- | --- |
| Some panes started elsewhere | Info, transient | All in the default directory: "3 panes started in your home folder because their folders no longer exist." All in an earlier saved folder: "2 panes started in an earlier folder because their last folders no longer exist." A mix: "3 panes started in a different folder because their last folders no longer exist." |
| Some panes or windows failed | Warning, persistent | "2 panes couldn't start and were left out: `<first error>`." |
| Saved state unreadable or a restore crashed | Warning, persistent | "Couldn't restore your previous windows: `<reason>`. The saved state was kept as runtime.json.corrupt." |
| Newer version | Warning, persistent | "Saved windows came from a newer Huterm and weren't restored. This window's sessions won't be saved." |
| Lock held | Info, persistent | "Another Huterm process is saving sessions, so this one won't restore or save its windows." |
| Write failed | Warning, persistent | "Couldn't save your windows for next launch: `<reason>`." |

### Server mode (design only, #45)

| Case | Terminals | What happens |
| --- | --- | --- |
| GUI layout restore | Live in the daemon | The GUI reconnects and applies `desktop.json` after its `scope_id` matches; nothing spawns. |
| Live reattachment | Live, same incarnation | The client reuses its ID mapping; windows reattach to their sessions. |
| Daemon restart | Lost; new incarnation | The daemon restores `runtime.json` with fresh shells and the saved numeric IDs under the same `scope_id`, so the GUI's layout still names valid sessions and tabs. |

Directory fill-in for server spawns stays with #45.

### Development and harnesses

- Every smoke, bench, and test harness sets `HUTERM_STATE_DIR` to a temporary
  directory, so none reads or writes the user's state.
- `mise run dev` sets `HUTERM_STATE_DIR` to a worktree-local
  `target/dev-state`, so a dev build neither restores nor overwrites the
  installed app's sessions.

## Pull request split

1. **Core and persistence (Refs #42).** Protocol capture types; core capture
   with directory candidates and retained launch directories;
   `Mux::restore` with ID preservation and the spawn report;
   `Mux::session_attached`; `huterm-state` (paths, lock, atomic writes,
   schema v1, bounds, validation, generation binding, corrupt handling) and
   the architecture check. No user-visible change; unused entry points carry
   `#[expect(dead_code)]`.
2. **Desktop restore (Closes #42).** `Persistence` and its states, window
   layout records with pane focus and window mode, Quit and checkpoint
   writes, bootstrap and restore, Quake and orphan handling, the setting and
   Settings row, notices, `HUTERM_STATE_DIR` in every harness and `mise run
   dev`, docs (`docs/configuration.md`, the runtime and desktop guides,
   GLOSSARY "Restoration"). UX review.

## Constraints

- `huterm-protocol` stays dependency-free; serialization stays out of it.
- Core alone owns PTYs and runtime state. Restore spawns on a worker, never
  the UI thread, and holds the Mux lock only to insert.
- Restore never executes saved text.
- Shutdown changes must still prove that live children and workers
  terminate, including children spawned by a cancelled restore.
- Tests use temporary state and config directories and a minimal
  allowlisted environment.

## Non-goals

- Scrollback and styled history (#43); command records; layout history.
- Lazy pane spawning; restoring running programs.
- Server mode, the daemon, reattach UI (#45, #21's server half, #46).
- A session switcher; restoring unviewed sessions without a window.
- A command-line flag to skip restore; the setting and the crash-loop guard
  cover it.
- View-local zoom (shared zoom is saved).
- A SIGTERM handler; checkpoints cover a killed process.

## Verification

| Concern | Evidence |
| --- | --- |
| Round trip of IDs, ordinals, order, names, splits, weights, zoom, input sync | `huterm-state` tests; core test comparing capture before and after restore |
| Restored IDs never collide with new allocations | Core test: restore, then create every kind; a watermark below the max ID is rejected |
| Terminated sessions and tabs do not reappear | GPUI test: close, checkpoint, load |
| Tabless transfer sessions are not saved | Core capture test; GPUI checkpoint during a held tear-off via the transfer hooks |
| Unviewed sessions restore and get a window | Core test with a detached session; GPUI bootstrap test |
| Quit differs from last-window close | GPUI tests: Quit writes the session; closing the last window writes an empty set before exit, without advancing the debounce clock |
| Write ordering | `Persistence` tests with a controllable writer: checkpoint paused before rename, then Quit; repeated termination; setting off with a write in flight |
| Generation binding | `huterm-state` test interrupting between the two renames; a layout against a replaced runtime file is ignored |
| Corrupt, oversize, deep, duplicate-ID, newer-version files | `huterm-state` tests: an oversize file is rejected before its body is read, an overlong list while parsing, and the rest at validation; renamed or left as the table says |
| Interrupted write keeps the old file | `huterm-state` test failing between temp write and rename |
| Second process cannot write | `huterm-state` test holding the lock from a child process; GPUI loser starts no shell |
| Crash-loop guard and Quit during restore | GPUI bootstrap tests with a controlled spawn hook; PTY test proves spawned children are reaped |
| Missing directory falls back and reports | Helper test plus PTY test spawning in a removed directory |
| Partial spawn failure keeps the rest | Core test with one failing command |
| A restored shell that exits at once honours `close_on_exit` | GPUI test for both settings |
| Quake stash: first summon uses the restored session; missing profile opens a regular window | GPUI tests |
| Setting changes | GPUI config-reload test: off deletes, on writes |
| Native restore | Smoke step on macOS and Linux: quit with two windows, a split, custom names, and a `cd`, relaunch with the same `HUTERM_STATE_DIR`, then assert names, order, weights, new child PIDs, each pane's reported directory, the focused pane, and settled window bounds and mode |
| Final write duration | Measured on both platforms and reported in the PR |
| Notices and the Settings row | UX review on the running app |
| Idle wakeups unchanged | `mise run bench:idle` before and after |

## Decisions settled

Jim accepted these on 2026-11-04:

1. State lives under `$XDG_STATE_HOME/huterm` (default `~/.local/state/huterm`)
   on both platforms, with a `HUTERM_STATE_DIR` override.
2. Crash checkpoints are written 1 s after changes stop.
3. `[general] restore_sessions`, default on; turning it off deletes saved
   state.
4. A second process on a held scope runs isolated with a notice.
5. A session without a saved window opens in a new window until a session
   switcher exists.
6. Restore is silent on full success; notices use the drafted copy, subject
   to UX review.
7. `mise run dev` uses a worktree-local state directory.
8. Two pull requests, as split above.
9. A pane whose shell had exited at Quit restores with a fresh shell.

## Open questions

None.
