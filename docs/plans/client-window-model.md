# Client window model

Status: planned for [issue #97][issue-97] and [issue #96][issue-96], delivered
in one PR.

Give the desktop client one model of its own windows. The model holds each
window's attachment, session, workspace, tabs with their terminal IDs and
display titles, active tab, activation history, quake profile summary, and
restorable layout. Any context reads it through the `Desktop` global without
touching a `WorkspaceView` entity or an `AnyWindowHandle`. Quake profile rows,
palette tab order, recent-tab selection, the Quit confirmation's tab titles,
and Quit capture all read the model. `quake_windows::Viewpoint` goes away.

Issue #96 is delivered inside #97: #97's scope already migrates "the quake
registry's live state", and both issues remove the same `Viewpoint` workaround.
A quake window is an ordinary `WorkspaceView` with a different presentation, so
the fact that a window serves a profile belongs in that window's record.

## Current state

Observed on `main` at `33a87782`, which is v0.15.1 (`4c12920f`) plus a
CI-only change, after the gpui-pre 0.3.6 upgrade and the dialogs, notices, and
window-chrome overhaul:

- `WorkspaceView` (`crates/huterm-gpui/src/desktop/windows.rs`) owns
  `attachment`, `workspace`, `active`, `history`, `bounds`, `sidebar_width`,
  `tab_scroll`, and `quake: Option<quake_windows::Presentation>`. The session is
  returned by `DesktopRuntime::open_tab` and discarded.
- `Desktop::windows` is a `Vec<WeakEntity<WorkspaceView>>`. It feeds
  cross-window reads (see the reader inventory below) and broadcast updates
  (`approved_quit`, `quake_windows::take_for_quit`, and the Linux
  `follow_button_layout` watcher). Config reload broadcasts through
  `cx.windows()` handles instead.
- `quake_windows::Registry::windows` maps profile names to `AnyWindowHandle`s.
- Navigation facts change at few sites: window open (`open_window_with_profile`),
  first attach (`new_tab` success), tab push, `select`, reorder
  (`finish_reorder`), and tab close. Tab close goes through
  `remove_closed_tabs`, which removes one tab (`CloseTarget::Tab`) or several
  (`CloseTarget::Tabs`, from Close Other Tabs and Close Tabs After), selects the
  surviving active tab, resumes queued closes, and requests window close when
  no tab remains. One test fixture also mutates navigation directly:
  `refresh_smoke::check_detachment` calls `remove_tab(&mut view.tabs, &mut
  view.active, ...)` to drop a tab view while keeping its core tab alive.
- Tab display titles come from `TabView::label` and `TabView::title`, which read
  the tab's `TerminalView` title, metadata, and exit state. `refresh_tab`, the
  per-tab activity drain for title, metadata, failure, and exit changes, already
  computes the label on every batch.
- Quake visibility changes inside `policy::Model` and is applied through
  `sync_quake_visibility`, which ends every quake transition path except some
  `step` error returns. `reconcile` only marks a window `detach_requested` when
  a reload removes its profile; `step` completes the detach later and removes
  the registry entry then. `invoke_now` refuses a summon while
  `detach_requested` is set.
- gpui-pre's `on_window_closed` callback receives the closed `WindowId`, and
  GPUI removes the window from `cx.windows()` before calling it.
  `gpui::WindowId` implements `From<u64>`, so pure tests can construct keys.
- Unit tests in `windows.rs` are pure functions without a GPUI test context.

## Reader inventory

Every production path that reads one window's facts from outside that window's
own view, or that reaches a window through its root view, with its disposition.
The PR description repeats this table with any additions found during
implementation.

| Reader | Reads | Disposition |
| --- | --- | --- |
| `quake_windows::profile_rows` (palette open and reload) | Other windows' quake state and tab counts through their view entities, using `Viewpoint` to skip the caller | Migrate to the model |
| `capture_windows` (native termination) and `capture_other_windows` plus the dispatching-view special case (`finish_close`) | Every window's `restore_window()` | Migrate to `restore_windows` |
| `WorkspaceView::close_dialog_input` (render, smoke state) | For Quit, sibling windows' tab IDs, terminal IDs, and titles through `Desktop::windows` | Migrate to the model; see "Tab titles" |
| `palette_tab_order`, `select_recent_tab` | The window's own history and tabs | Migrate to the model, as #97 names them |
| `modal_focus`, `modal_showing`, `terminal_input_allowed`, `active_composition` | The window's own root view: dialog, busy, reorder, palette, menu, and composition state | Stay. These are ephemeral presentation state, read by that window's own terminal handlers |
| `observe_keystroke` (global keystroke observer) | The dispatching window's own root view: busy, dialog, reorder, palette, and its active `TerminalView` entity | Stay. It runs inside that window's dispatch and needs the entity, not a model fact |
| `Desktop::invoke`, `Route::Terminal` | The target window's active `TerminalView` entity, inside that window's own `handle.update` | Stay, for the same reason |
| `Desktop::invoke`, `Route::Application` | A `WeakEntity` reporter for the target window | Stay. It obtains a handle for later notices, not a fact |
| `Desktop::invoke`, `Route::Window` | The target window's root view, to run a window command inside its own `handle.update` | Stay. A targeted command update, not a read of another window |
| `quake_windows::invoke_now` | The profile's window, to request summon or hide on its `Presentation` | Stay as a targeted update. It finds the window through `WindowModel::quake_window` instead of `Registry::windows` |
| `quake_windows::recover` and the deferred update in `reconcile` | The quake window's `Presentation`, to recover from a native failure or begin a detach | Stay as targeted updates. `reconcile` finds its windows through the model |
| `quake_windows::work::complete` | The quake window's `Presentation`, to clear or record native-effect failures | Stay as a targeted update |

Targeted updates act on one window's own view to run a command or maintain its
presentation; they read no fact another window owns. Broadcast updates write
to every window and stay on the weak view list: `approved_quit`,
`take_for_quit`, `follow_button_layout`, and config reload.
`show_active_window_failure` and `request_quit` are targeted updates of the
active window.

## Scope

In scope:

- A pure `WindowModel` in `crates/huterm-gpui/src/desktop/windows/model.rs`,
  held by `Desktop`, with named transitions and reader functions.
- Moving attachment, session, workspace, active tab, and activation history
  ownership out of `WorkspaceView` into the model, plus published tab titles.
- Moving the quake profile-to-window association into the model and mirroring
  quake visibility from `Presentation`.
- Migrating every reader marked "Migrate" above. Removing `Viewpoint`,
  `capture_other_windows`, and the dispatching-view capture special case.
- Smoke assertions, agent guidance, and plan status updates.

Out of scope, with the owning issue:

- Runtime hierarchy projection, tab-bar rename propagation, and removing the
  palette's background hierarchy fetch (`load_palette_hierarchy`): #98. Tab
  titles in the model reflect exactly what the tab bar shows today, including
  its stale custom names after a rename.
- Pane layouts (#28), panel placement (#27), durable layout records (#42),
  server-mode attachment and session switching (#45, #26), and shared viewers
  (#22). The model leaves room for these fields but does not define them.
- Caching tab-bar label layout for rendering (#164). The published title is a
  model fact, not a render cache.
- Moving existing inline test suites (#146). New model tests use an adjacent
  child module; nothing else moves.
- Diagnostic readers in smokes and benchmarks (`quake_smoke`, `palette_smoke`
  window dumps, `input_smoke`, `idle_bench`, `fullscreen_smoke`). They inspect
  view internals by design, from contexts with no window update on the stack.
  The one production-shaped call among them, `palette_smoke::quake_state`,
  migrates to the model.

## Decisions

### The model owns navigation facts; the view keeps entities and presentation

`WorkspaceView` loses its `attachment`, `workspace`, `active`, and `history`
fields and gains `window: WindowId`. It reads these facts through a
`record(cx)` accessor and changes them only through model transitions. A single
store cannot drift, and it is the shape #98 needs: projection events will update
the model directly, and views re-render from it.

The alternative, a view-owned mirror published to the model, needs less churn
but reintroduces drift and makes #98 route every structural event through the
owning view. The cost of the chosen option is mechanical: about 44 production
reads of `self.active`, `self.workspace`, `self.attachment`, and
`self.history` in `windows.rs` gain a `cx` lookup, and some `&self` helpers such
as `active_view`, `presentation`, `current_close_target`, and `restore_window`
gain a `cx: &App` parameter. Copy scalar facts out of the record before any
`cx.notify()` or `cx.global_mut()` call.

### Tab list alignment

The view keeps `tabs: Vec<TabView>`, because it owns the `TerminalView`
entities and activity tasks. The model keeps one entry per tab in display
order: tab ID, terminal ID, and published title. Three view helpers are the only
code that changes either list, and each updates both in one call:

- `push_tab_view` after a successful spawn;
- `apply_tab_view_order` for canonical reorder, which the model accepts or
  rejects before any view moves;
- `drop_tab_views(ids)`, which removes tab views and model entries for one or
  several tabs, applying the current next-active rule per removed tab and
  pruning history.

`remove_closed_tabs` calls `drop_tab_views` and keeps its follow-up: select the
surviving active tab, resume queued closes, and request window close when
empty. `refresh_smoke::check_detachment` calls `drop_tab_views` alone, because
it must keep the core tab alive without closing the window. A `debug_assert` in
each helper checks that view and model orders match.

`view.tabs.len()` remains fine for the view's own rendering. Readers outside
the view use the model's tab list.

### Tab titles

The model entry's `title` is the string `TabView::title` returns today: the
resolved label plus the exited suffix. Publish it:

- at `push_tab_view`, from the new tab's initial state;
- in `refresh_tab`, after the terminal refresh, only when the resolved string
  differs from the published one;
- in `apply_reload`, for every tab, because label settings change the result.

`refresh_tab` already computes the label for notices on every batch, so the
added cost is one string comparison, plus a write when the title changes.
`close_dialog_input` then reads every tab's title and terminal ID from the
model, for its own window and for Quit's sibling windows alike, which removes
its view-entity loop.

### Quake keeps its presentation and mirrors one fact

Decided: the quake profile state lives in the window model, not in
`quake_windows::Registry` as #96 originally proposed; #96 now reflects this.
The reasons are one owner for the profile association and no second write path
for the tab count.

`Presentation` keeps the native window, geometry, display, animation stage,
fullscreen and regular state, focus, and native handles, as #96 requires. The
model record holds `quake: Option<QuakeRecord { profile: String, visible: bool
}>`. The tab count comes from the record's tab list.

`visible` is the desired visibility, the target of the show or hide transition,
not whether the window is on screen yet. It mirrors `Presentation::visible()`.
Publish it from `sync_quake_visibility` and after every `step` return. `step`
can change visibility inside `policy::Model::decide`, for example through
auto-hide on focus loss, and then return early on a later failure without
reaching `sync_quake_visibility`. Wrap `step` so that one publication runs on
every return path, rather than adding a call before each `return`.

Both call sites use one pure helper that copies the summary from a
`policy::Model` into the window model. Policy tests already construct
`policy::Model` without native state, so the helper is unit-testable against
real policy transitions rather than a hand-set flag.

Toggling a quake window into temporary regular presentation keeps its quake
record: the window still serves its profile. The record's `quake` field is
cleared only when the window stops serving the profile:

- when `step` completes a detach that `reconcile` requested after a reload
  removed the profile. `reconcile` itself only reads the model to find the
  window and sets `detach_requested`. Until `step` completes, the association
  stays, so `invoke_now` keeps refusing a summon for a removed and re-added
  profile instead of opening a second window;
- on window close;
- at Quit.

### The model replaces the registry's window map

Remove `Registry::windows`. The model records the profile association when the
window opens and clears it in the three cases above. `invoke_now` finds the
window ID with `WindowModel::quake_window(name)` and resolves the handle from
`cx.windows()`. That list includes windows currently on the update stack;
`invoke_now` runs deferred, so no update is in progress. Every current
`Registry::windows` user migrates: `invoke_now`'s lookup and its post-creation
check, `attach`, the `step` detach completion, `close`, `reconcile` (read
only), and `take_for_quit`. The registry keeps the platform, hotkey
registrations, return focus, failed-spawn latch, and smoke journal.

### Record lifetime and attachment

- `open_window_with_profile` inserts the record inside the `open_window`
  callback, before `cx.new` creates the view, with the quake summary when
  `quake_windows::attach` succeeded.
- `attach` records the attachment, session, and workspace at the first
  successful spawn.
- `detach_attachment` clears the attachment and session whenever
  `finish_close` accepts a window close and proceeds to `remove_window`: on
  `Ok`, and on a teardown error. `Mux::close_session` removes the attachment
  and the session's structure before terminal cleanup can fail, and
  `finish_close` already reports that error and removes the window, so the IDs
  are invalid in both cases. `StaleClose` and `ConfirmationRequired` retry the
  close and keep the attachment, as does an assessment failure in
  `request_close`, which cancels. A window with no attachment goes straight to
  `remove_window` and needs only begin-close and removal. Extract the
  retry-or-accept decision on the commit result into a pure function that both
  `finish_close` and its tests use. The embedded desktop has no other detach
  path today; #26 session switching will reuse this transition before
  attaching elsewhere. Application Quit does not detach individual windows:
  capture takes the records as they are.
- `remove_window` marks the record `closing`. Closing records are excluded from
  profile rows, Quit dialog titles, and Quit capture, and their quake
  association is cleared at the same time as the current `quake_windows::close`.
  The view can still read its own record while macOS finishes adapter cleanup.
- The `on_window_closed` callback drops the record for the `WindowId` it
  receives, then keeps its current `maybe_exit` call. This also covers windows
  removed without `remove_window`, such as a failed quake creation.
- `record(cx)` returns `None` when no record exists, and callers treat that as
  absent state rather than a fallback record. That happens only after the
  window has been removed, when a queued weak-entity update still runs.

### Session

Record the session returned by `open_tab` at first attach. The desktop does not
retarget attachments or move workspaces between sessions yet, so the value stays
correct until #98 updates it from events. Do not change palette target loading
in this PR; that belongs to #98.

### Restore capture

`WindowModel::restore_windows` builds the Quit capture from records alone, in
window-open order. It replaces `capture_windows`, `capture_other_windows`, and
the dispatching-view special case in `finish_close`. The record's layout holds
the restorable `bounds` and `sidebar_width`. Write bounds where `self.bounds`
is written today: `observe_fullscreen` (which both the fullscreen work loop and
the toggle commands call), `finish_close`, and the quake `step` paths. Write
the sidebar width in `resize_sidebar`. `tab_position` comes from the global
config at capture time, as today.

Add the quake profile name to `WindowRestore`, because the quake plan reserves
this association for #42.

Decided: drop `tab_scroll` from `WindowRestore`. It is local framing, changes
per animation frame at six sites, and a restored window reveals its active tab
anyway. The capture is in-memory only, with no serialization contract, so
dropping it has no compatibility cost. The view keeps `tab_scroll` as ordinary
presentation state.

### Keeping the weak view list broadcast-only

Move the weak view list behind a small `Desktop` API, such as
`broadcast(cx, |view, cx| ...)`, whose closure returns nothing, and make the
field private to that module. Rust cannot stop a closure from copying a fact
out, so this is a guard against casual reads, not proof. The reader inventory
above, the review-time search below, and the overlays check's Quit-dialog
assertion cover the rest.

## Model shape

Illustrative, not binding on field names:

```rust
//! Client-owned facts about this client's windows. Not yet covered, for
//! later extension: pane layouts (#28), panel placement (#27), and durable
//! layout records (#42).
pub(super) struct WindowModel {
    records: Vec<WindowRecord>, // window-open order
}

pub(super) struct WindowRecord {
    pub id: WindowId,
    pub attachment: Option<AttachmentId>,
    pub session: Option<SessionId>,
    pub workspace: Option<WorkspaceId>,
    pub tabs: Vec<TabEntry>,    // display order: id, terminal, title
    pub active: Option<TabId>,
    pub history: Vec<TabId>,    // most recent first
    pub quake: Option<QuakeRecord>,
    pub layout: WindowLayout,   // restorable bounds, sidebar width
    pub closing: bool,
}
```

Transitions, each unit-tested: `open`, `attach`, `detach_attachment`,
`open_tab` (push and activate), `select`, `apply_order`, `close_tabs` (one or
several IDs; keeps the current `remove_tab` rule for the next active tab,
applied per removed tab, and prunes history), `set_tab_title`,
`set_quake_visible`, `detach_quake`, `set_layout_bounds`, `set_sidebar_width`,
`begin_close`, and `remove`.

Readers, each a function of `&WindowModel` plus configuration: `record`,
`quake_window(name)`, `quake_state(name)`, `palette_tab_order(id)`,
`recent_tab(id)`, `tab_titles(scope)` (one window, or every open window for
Quit), and `restore_windows(tab_position)`. `quake_windows::profile_rows(cx)`
is an adapter, not a model reader: it combines the configured profiles with
`quake_state`.

`Desktop` holds `windows: WindowModel`. The weak view list moves behind the
broadcast API above.

## Implementation steps

1. Add `windows/model.rs` with the model, transitions, and readers, and its
   tests in the adjacent `windows/model/tests.rs`. Move
   `record_tab_activation`, `prune_tab_history`, `recent_tab`, `remove_tab`,
   and `apply_tab_order` logic and their existing tests into it. Write the
   module documentation, including the non-coverage list.
2. Add the model to `Desktop` and put the weak view list behind the broadcast
   API. Insert records at window open and remove them in `on_window_closed`.
3. Move navigation ownership. Add `WorkspaceView::window`, the `record(cx)`
   accessor, and the three tab-list helpers. Remove the four fields and update
   their readers. Record the session at first attach and detach the attachment
   at window-close commit. Migrate `refresh_smoke::check_detachment` to
   `drop_tab_views`. Run `mise run check` and the focused `huterm-gpui` tests
   before continuing, since this step carries most of the churn.
4. Tab titles: publish at push, in `refresh_tab`, and in `apply_reload`; switch
   `close_dialog_input` to the model.
5. Quake: record the association at open; publish visibility from
   `sync_quake_visibility` and the `step` wrapper; clear it at `step`'s detach
   completion, `close`, and `take_for_quit`; make `reconcile` read the model.
   Replace `Registry::windows` with model lookups. Change `profile_rows` to read
   the model and config, delete `Viewpoint`, and drop the calling-view parameter
   from `quake_profile_rows`.
6. Readers: switch palette tab order, `select_recent_tab`, and both Quit capture
   paths to model readers. Write layout facts at their current write sites.
   Delete `capture_other_windows` and the special case in `finish_close`, and
   drop `tab_scroll` from `WindowRestore`.
7. Smokes and documentation, as described in the next two sections.

Steps 3 to 5 all edit `windows.rs` and `quake_windows.rs`, so do them in
order.

## Verification

Unit tests, all pure and without GPUI, in `windows/model/tests.rs`:

- Each transition listed in the model shape, including the #97 list: window
  open, attach, detach, tab activation, tab open and close, quake summon and
  hide, and window close.
- Profile rows report `NotSummoned`, `Hidden { tabs }`, and `Visible` after
  summon, hide, tab open and close in a quake window, temporary regular
  presentation (still associated), a pending detach (still associated), detach
  completion, begin-close, and removal.
- The publication helper, driven by real `policy::Model` transitions: a summon
  request, a hide request, `decide` auto-hiding on blur after the suppression
  deadline, a fullscreen toggle (`regular` flip plus `request(true)`), and
  recovery. Each test asserts the resulting profile row, so a transition that
  changes policy visibility without publishing fails. The step wrapper's
  structure and the quake smoke cross-check below cover the wiring.
- `close_tabs` preserves the current next-active rule and prunes history for
  one tab, several tabs (Close Other Tabs and Close Tabs After shapes), and the
  active tab among several. Reordering rejects stale or mismatched orders.
  `palette_tab_order` and `recent_tab` keep their current results.
- The commit-outcome function retries on `StaleClose` and
  `ConfirmationRequired`, and accepts `Ok` and teardown errors such as
  `MuxError::Runtime(RuntimeError::ShutdownTimedOut)`, so `detach_attachment`
  runs exactly for accepted closes.
- `tab_titles` returns one window's entries or every non-closing window's for
  Quit, with the published titles, and the existing
  `close_dialog_input_maps_busy_terminals_to_tabs` cases pass when fed from it.
- `restore_windows` excludes closing records and returns the same records
  whether or not any view exists. Port
  `quit_captures_zero_view_hierarchy_and_window_navigation_once` to build its
  windows through the model. The production bounds publication in
  `observe_fullscreen` is checked natively by the fullscreen smokes below.
- Reader independence: one test builds a model with no GPUI context and calls
  every reader. Their signatures take `&WindowModel`, so a reader cannot reach
  a view. This proves the readers need no view; the inventory and the search
  below prove the consumers use them.

Native smoke coverage:

- Palette smoke: add the quake picker's row details to the palette smoke state,
  such as `profiles=default:visible,logs:not-summoned`. In
  `scripts/check-palette.ts`, assert them when the palette opens inside the
  quake window (the summoning window reports itself as visible) and from window
  0. Run `smoke:macos-palette` and `smoke:linux-palette`.
- Quake smoke, model cross-check: emit the model's per-window quake summary and
  tab entries next to the native state in `quake_smoke`, and have
  `scripts/check-quake.ts` assert on every state read that the model's
  `visible` matches the native `desired` state and that its tab count and order
  match the view's `wN.tabs`.
- Quake smoke, tab count: add steps while the default profile is associated.
  Open two more tabs, hide the window, and assert `default` reads
  `Hidden { tabs: 3 }` in both the model summary and `quake-state`. Summon it
  and run Close Tabs After from the first tab. Then open one replacement tab and
  close that replacement with `close_tab`, so the original tab, its PTY, and
  the window survive for the later steps. Every fixture shell holds a
  background job, so each close shows a confirmation: wait for the assessment
  and confirm it explicitly. After each removal, assert the surviving tab IDs,
  order, count, and that the profile association remains. The existing steps
  already drive summon, hide, auto-hide, fullscreen, regular conversion,
  profile removal, and close. The quake smoke keeps only the model cross-check
  and tab-count steps, which add no activation cycles to its focus-sensitive
  steps.
- Overlays check, Quit dialog titles: serialize the close dialog's job groups,
  with their terminal IDs and mapped titles, as `wN.dialog_groups` in the
  palette smoke state. Add an `activate\t<index>` palette smoke command
  generalizing `activate-first`. When `HUTERM_PALETTE_SMOKE` is set,
  `apply_reload` records the titles it has just published, synchronously at the
  end of its republication, as `reload_titles`. Add a final section to
  `scripts/check-overlays.ts`, after its existing sections and before its
  closing `quit` command. It uses two ordinary windows, so it needs no quake
  activation. The overlays fixture already provides `busy` and `title`
  commands and configures `tabs.label = "title"`.
  1. Run `open-second` and wait until window 1's terminal is focused. Type
     `busy` and `title siblingone` into it, and wait for its window title.
  2. Activate window 0 and wait for its `active` state. Invoke `quit` against
     window 0, which defers `request_quit` to the active window. Wait for
     `w0.confirming=true`, assert that `w0.dialog_groups` holds window 1's
     terminal ID with the title `siblingone`, and cancel. Window 0 hosts the
     dialog, so that title can only come from window 1's record.
  3. Activate window 1, type `title siblingtwo`, wait for its window title, and
     repeat step 2 expecting `siblingtwo`. Only `refresh_tab` publication
     supplies the new title.
  4. Rewrite the config with `tabs.label = "directory"` and reload. Assert that
     `reload_titles` shows window 1's tab with a label different from
     `siblingtwo`. Only reload publication writes that record, so later
     `refresh_tab` activity cannot mask a missing one. The fixture script is
     named `shell`, which `huterm-procinfo` does not recognize as a shell, so
     only a label mode that ignores the title proves a change. If directory
     metadata is missing, the label falls back to the title and the step fails
     with that diagnostic rather than passing without evidence. Restore the
     original config and reload.
- Fullscreen smokes: emit the model's layout bounds and sidebar width as
  `wN.model_restore` and `wN.model_sidebar` in `fullscreen_smoke`, next to the
  existing `wN.restore` (the controller's restorable bounds). Add a smoke
  command that resizes window 0 through `Window::resize` while windowed. In
  `scripts/check-fullscreen.ts`, inside the default-mode block that already
  runs `checkReserved` (skipped for `noWm`, `frameProbe`, `fallback`, and
  `schedulerOnly`): record the initial `w0.restore`, resize, wait until
  `w0.restore` reports the new size, and assert that `w0.model_restore` equals
  it. Then resize back to the initial size and check again. Hosted macOS 15
  restored native fullscreen to the launch frame rather than the resized one,
  so the existing restoration baselines stay at the launch size. The resize
  reaches the model
  only through the bounds observer, the fullscreen work wake, and
  `observe_fullscreen`, so a missing publication leaves the old value and
  fails. Repeat the equality check after entering and leaving fullscreen, and
  compare `w0.model_sidebar` after the sidebar drag inside `checkReserved`. All
  of these reads precede `finish_close`, whose own bounds write cannot mask a
  gap. Keep the existing Quit assertion, which runs the assessed Quit path
  while fullscreen and checks the captured bounds. Run
  `smoke:macos-fullscreen` and `smoke:linux-fullscreen`.
- Refresh smoke: `check_detachment` must still prove that dropping a tab view
  cancels its activity without a runtime wake, now through `drop_tab_views`. Run
  `smoke:macos-refresh`.
- Input smokes: both input smokes quit through a real key binding (`cmd-q` on
  macOS, `ctrl-shift-q` on Linux) and run `reload_config` through native keys.
  Both commands pass through the `InvokeApp` global handler and then reach
  model reads: Quit's close path captures from the model, and reload calls
  `quake_windows::reconcile` and republishes titles. Run `smoke:macos-input`
  and `smoke:linux-input` unchanged.
- Run macOS smokes through `mise run vm:macos:smoke` so they do not take over
  the host display.

Static and suite checks: `mise run check` during steps 3 to 6, then
`mise run test` and `mise run verify` before handoff. Linux runs through the
Docker tasks (`mise run linux:smoke`) or CI.

Read contexts required by #97. Model reads go through `cx.global::<Desktop>()`,
which does not touch the window stack, so the risk is a caller that follows a
model ID back into a window. Evidence for each context:

| Context | Production reader | Evidence |
| --- | --- | --- |
| A window's own action handler or render | Profile rows while opening the palette inside a quake window; `select_recent_tab`; the Quit dialog's titles during render | Palette smoke opens the quake profile picker inside the quake window, on both platforms, and asserts its rows; the overlays check asserts the Quit dialog's titles |
| A global action handler | `InvokeApp` handlers, which defer before reading window facts: Quit defers `request_quit`, and quake commands defer `invoke_now` | Both input smokes quit and reload through native key bindings |
| An async task or App callback | Global-hotkey dispatch reaching `invoke_now`; Quit capture in `finish_close` | Quake smoke global shortcuts; fullscreen smoke Quit capture |

No production global action handler reads window facts synchronously today. A
dispatch-window borrow regression in these paths panics, and GPUI aborts the
process, so the smoke fails. If a handler later reads window facts
synchronously, it can read the model directly but must not resolve a model ID
into the dispatching window's handle.

Coverage limits: the native termination capture in `on_app_quit` has no
dedicated smoke; it calls the same `restore_windows` reader that the unit tests
cover. The input smokes' Quit steps assert clean exit, not capture contents.
`smoke:macos-quit` runs a separate application without `Desktop` and does not
exercise this plan's paths.

A GPUI `TestAppContext` harness would test these contexts in-process, but the
repository has none and building one for a `WorkspaceView` needs the runtime,
fonts, and platform globals. That cost is out of proportion to a risk the
smokes above already exercise.

Review evidence for the PR: search production code (excluding smoke and
benchmark modules) for `downcast::<WorkspaceView>`, `root::<WorkspaceView>`,
`WeakEntity<WorkspaceView>` upgrades followed by `read`, and uses of the
broadcast API. Every hit must match a "Stay" row or a broadcast in the reader
inventory; record the result in the PR.

## Documentation

- `AGENTS.md`: replace the `Viewpoint` rule ("Cross-window reads such as quake
  profile rows take a `Viewpoint`...") with: cross-window and application reads
  of window facts use the window model; the weak view list is for broadcast
  updates only; never read another window's view entity or handle for facts.
  Keep the existing warning about reading the dispatching window.
- `CONTEXT.md`: add "Window record", the client-owned facts for one window. Note
  that client layout for #42 is built from window records, and that pane
  layouts and panel placement are not yet part of them.
- `docs/plans/quake-profiles-global-hotkeys.md`: point the registry description
  at the window model.
- This plan: set its status when the PR lands.

## Risks

- Reads during teardown: a queued weak-entity update after window removal sees
  the empty record. Readers already treat a missing workspace or active tab as
  unavailable, so they refuse the command rather than act on stale IDs.
- Global access while the view is updating: `cx.global_mut::<Desktop>()` inside
  a view update is safe because nothing in the desktop leases `Desktop` through
  `update_global`. Keep it that way; the plan relies on it.
- Tab-list alignment: the `debug_assert` in the three helpers catches a
  divergent path in debug builds and tests. A release build would show a
  palette order, quake tab count, or dialog title out of step with the tab bar,
  not a crash.
- Title staleness: a published title can lag the tab bar only if a label input
  changes outside `refresh_tab`, push, and reload. Core renames are one such
  input today and stay stale in both places until #98.
- Behavior drift in navigation: keep the existing selection and close rules
  exactly in the model, including multi-tab removal order, and port their tests
  before changing call sites.
- Merge churn: `windows.rs` is about 10,900 lines and changes often. Commit the
  pure model (step 1) separately and rebase before step 3's mechanical edits.

[issue-96]: https://github.com/jimeh/huterm/issues/96
[issue-97]: https://github.com/jimeh/huterm/issues/97
