# Embedded session close

Status: proposed for the embedded half of [issue #21][issue-21].

Closing, detaching, switching, and quitting are separate lifecycle events.
This plan covers them in embedded mode, where the desktop owns the runtime
in-process. It records the rules, closes the gaps that remain on `main`, and
maps the issue's acceptance criteria to evidence. The server-mode half
of #21 (Detach, Terminate Session, and Cancel in the close prompt; GUI Quit that
leaves a daemon running) is verified with [#45][issue-45] and is not planned
here.

Parent epic: [#9][issue-9]. The dependency [#20][issue-20] has landed, and
so have [#97][issue-97] (client window model), [#98][issue-98] (hierarchy
projection), and [#22][issue-22] (terminal viewers), which delivered most of
the embedded lifecycle. [#41][issue-41] (quake profiles), which this stage
was meant to unblock, is already closed.

## Current state

Observed on `main` at `fe683573`. Most of the embedded half is already
implemented:

- `Mux::prepare_close` (`crates/huterm-core/src/mux/lifecycle.rs:245`)
  resolves `CloseRequest::Window` under the Mux lock: `Detach` when another
  attachment holds the session, otherwise `Session`. A detach ticket covers
  no terminals, so it never needs confirmation.
- `CloseTicket::check_jobs` runs off the UI thread and outside the Mux lock.
  `validate_close_at` (`lifecycle.rs:421`) rejects a commit when the Mux
  revision moved, when the job evidence widened, or when the assessment is
  older than two seconds. Idle shells assess as `JobState::Idle`; foreground
  jobs, background groups, orphans holding the TTY, and unknown process
  state need consent.
- `detach_session` and `retarget_attachment` revoke the attachment's viewers
  and keep the session, including with zero views. `close_session` revokes
  every attachment of the session and stops all its terminals.
- The desktop close pipeline (`CloseState` and `request_close`,
  `finish_close`, and `remove_closed_tabs` in
  `crates/huterm-gpui/src/desktop/windows.rs`) assesses, confirms, and
  commits. A refused commit (`StaleClose`, `ConfirmationRequired`) assesses
  again. The native close button (`on_window_should_close`, `windows.rs:2047`),
  the menu, and shortcuts all reach `request_close(CloseTarget::Window)`.
  Dock Quit and AppKit terminate reach `request_quit` through
  `native_quit.rs`.
- Quit captures every session, including unviewed ones, and every open
  window's layout into `DesktopRuntime::restore` before teardown, once
  (`DesktopRuntime::commit` and `terminate_with_windows`). The capture is
  in-memory only; the durable writer is [#42][issue-42].
- Hiding a quake window never calls the close pipeline, and hiding is
  refused while a close confirmation shows. `smoke:macos-quake` and
  `smoke:linux-quake` check that hidden quake windows keep their PTYs.
- Core tests cover detach keeping a real child alive, last-window close
  reaping it, two concurrent `Window` tickets, structural changes
  invalidating consent, background and orphan jobs, and Quit including
  zero-view sessions.

The gaps:

1. **Externally emptied windows.** Reconcile drops a window's views when
   another client closes its tabs, moves them away, or closes its session
   (`removals`, `crates/huterm-gpui/src/desktop/windows/projection.rs:318`).
   `drop_tabs` never closes the window, so it stays open with no tabs. When
   its session closed, its window record still names an attachment that
   core has invalidated. The window's own last-tab follow-up has a related
   race: `remove_closed_tabs` (`windows.rs:6691`) requests a full
   `CloseTarget::Window` close whenever `self.tabs` is empty, even while a
   tab that another client just opened in the workspace is still installing
   (`finish_install`, `desktop/windows/view_install.rs:265`). For the last
   attachment that close terminates the arriving tab, without a prompt when
   it is idle.
2. **A stale Confirm click.** The Confirm button's listener
   (`windows.rs:8376`) calls `finish_close` with the target captured at
   render. A click dispatched against a frame drawn before Cancel finds no
   assessment, so `finish_close` calls `request_close` and reopens the close.
   If the jobs ended meanwhile, that close commits without a prompt. Cancel
   and Escape already check `can_cancel_confirmation`; Confirm does not.
3. **Attachment events.** The hierarchy stream carries no attachments.
4. **`close_on_exit` with several clients.** The policy lives in each
   window's config.
5. **Evidence.** No PTY test proves that a retarget (switch) keeps a real
   child alive, or that terminating one session spares a sibling session's
   child. No tier 2 test runs two windows of one session through concurrent
   window closes. No smoke drives the native close button or AppKit
   terminate through the desktop: the smokes use catalog commands, and
   `smoke:macos-quit` runs the AppKit bridge against a standalone runtime.
6. **Stale docs.** The README says process inspection uses a `ps` snapshot.
   It has used `huterm-procinfo` in-process since #160.

## Scope

This plan delivers:

- the lifecycle rules below, written into the README and the desktop guide;
- closing an externally emptied window;
- the Confirm guard;
- the missing tests and native smoke steps; and
- decisions on attachment events and `close_on_exit`.

Deferred, with owners:

| Item | Owner | Why |
| --- | --- | --- |
| Close prompt with Detach, Terminate Session, and Cancel | #21 server half, verified with #45 | Embedded mode has no daemon to detach into. |
| GUI Quit that leaves sessions running | #21 server half, #45 | In embedded mode Quit ends the runtime. |
| Attachment events in the hierarchy stream | [#26][issue-26] | See the decision below. |
| Runtime-owned `close_on_exit` | #21 server half, delivered with #45 | See the decision below. |
| Durable restore writer and restore on launch | #42 | The capture already exists; persistence is separate. |
| Reaching an unviewed session after its last window closes | #26 | Nothing in embedded mode creates one until switching exists. |
| An explicit Terminate Session command | #26 or #45 | Closing the last window already terminates; a picker is where a separate command belongs. |

## Lifecycle rules

Each row is one event in embedded mode. "Prompt" means a confirmation when
the assessment finds a foreground job, a background group, an orphan on the
TTY, or unknown process state. An idle shell never prompts.

| Event | Trigger today | Runtime effect | Window effect | Prompt |
| --- | --- | --- | --- | --- |
| Close a tab, others remain | `close_tab`, tab button, tab menu | `close_tab` stops and reaps the terminal | View dropped, next tab activated | Yes, for that tab |
| Close a window's last tab | Same | As above, then an emptied-window close | See the emptied row | For the tab only |
| Close a window, another attachment holds the session | `close_window`, title-bar button, WM close, menu | `detach_session`: viewers revoked, terminals untouched | Window removed | Never: no terminal is affected |
| Close a window, last attachment | Same | `close_session`: every terminal stopped and reaped | Window removed; the app exits when no window, no session, and no quake hotkey registration remain | Yes, across every terminal in the session |
| Root shell exits, `close_on_exit = true` | Shell exit | One window commits `close_tab` | Tab removed; last tab closes the window | Never |
| Root shell exits, `close_on_exit = false` | Shell exit | None; the child is reaped, history kept | Exited tab retained | Not applicable |
| Window emptied (new) | Its last tab closed here or by another client, moved elsewhere, or exited | `CloseRequest::EmptiedWindow`: refused while Mux holds tabs in the window's workspace; Detach while another attachment holds the session or the session keeps terminals elsewhere; otherwise deletes the empty session | Window removed, or kept when refused | Never: it stops no terminal |
| Window's session closed by another client (new) | `SessionClosed` | The same request fails with `UnknownAttachment`; core already invalidated the attachment | Window removed without a notice once its close pipeline is idle | Never |
| Switch a window to another session | Fixture only until #26 | `retarget_attachment`: viewers revoked, both sessions survive | Window shows the new session | Never |
| View destruction | Dropping a `TerminalView` | Viewer dropped; attachment and terminals untouched | None | Never |
| Quake hide or show | Hotkey, `toggle_quake` | None | Hidden or shown; attachment and PTYs kept | Never; hiding is refused while a confirmation shows |
| Quit | Menu, shortcut, Dock, AppKit terminate | Capture restore set, then `shutdown` of every session, viewed or not | All windows removed | Yes, across every terminal |
| Native termination after consent | `on_app_quit` | Capture (first capture wins), then `shutdown` | Process exits | No; consent was given or the OS is terminating |

Rules that hold across rows:

- Close intent lives in desktop commands, never in view destructors or
  attachment removal.
- The runtime decides Detach versus Session under its lock. The client never
  decides it from its own view of attachments, which can be stale.
- Ordinary close and termination remove sessions before any Quit, so they
  never appear in a later capture. Quit replaces nothing on disk yet (#42).
- A zero-view session survives until Quit. Today no production path creates
  one, because each window owns a private session and nothing retargets.
  An emptied close creates one only for a session with several workspaces,
  which no production path creates either.
- Automatic closes, emptied windows and exit-closes, run only while the
  window's close pipeline is idle: not busy and no confirmation showing.
  They wait behind queued user closes and never remove a window that hosts
  an Application close.

## Decisions

### An emptied window closes without stopping a terminal

A window whose last tab view goes away closes as an automatic request,
`CloseTarget::Emptied`, which maps to a new core request,
`CloseRequest::EmptiedWindow { attachment, workspace }`. This matches the
local last-tab rule and tmux's default `detach-on-destroy on`. Keeping an
empty window open would leave a view with nothing to show and no way to
choose a session until #26. Retargeting to another session belongs with the
session switcher in #26, which may revisit this rule.

`prepare_close` resolves the request under the Mux lock:

1. An invalidated attachment fails with `UnknownAttachment`.
2. When the workspace still exists in the attachment's session and holds
   tabs, the request fails with a new `MuxError::ViewNotEmpty`. The window
   keeps running, and reconcile installs the tabs the projection has not
   shown yet.
3. When another attachment holds the session, the effect is Detach.
4. When the session still holds terminals in other workspaces, the effect
   is Detach too, which leaves the session unviewed rather than terminating
   terminals the window never showed.
5. Otherwise the effect is Session, which deletes the empty session.

Every resolution stops no terminal, so the request never needs consent.
The commit's revision check covers changes after the assessment: an
`open_tab` or move bumps the revision, the commit returns `StaleClose`, and
the retry prepares the request again. The decision has to be canonical. A
client-side check against the projection was considered and rejected: the
projection can lag Mux, a tab that is still installing has no view, and a
`Window` assessment covers terminals in every workspace of the session, so
the client cannot tell an arriving tab from one moved to a sibling
workspace.

The trigger is an edge. A window requests an emptied close when:

- a reconcile removal took its last view;
- its own close or exit-close removed its last tab, replacing the
  `CloseTarget::Window` follow-up in `remove_closed_tabs`; or
- the projection no longer holds its attached session.

New windows before their first spawn, `attach-window` fixture windows
before their installs, and the no-PTY Quit host have no views to remove and
an attached session that still exists, so they never request one.

The emptied close is automatic, like an exit-close. `next_request` takes it
after queued user closes and only while no confirmation shows. A user's
Window or Application close absorbs it, and it never prompts. A tab
confirmation whose tabs all left the window is cancelled first, since its
commit could only go stale. Reconcile only marks the window emptied; it
never removes a window itself, so no assessment or commit continuation runs
after its window is gone.

Outcomes:

- Detach or Session: the window clears its attachment in the model and is
  removed, as a committed `Window` close is today.
- `ViewNotEmpty`: the window stays and remains marked emptied until a view
  installs or an emptied close succeeds. While marked, every hierarchy batch
  that touches its workspace, and every install completion that installs
  nothing, requests the emptied close again, but only when the window shows
  no view and the projection holds no tab in its workspace. A tab still
  installing therefore never triggers a retry, and one that vanishes before
  it installs does. A tab whose install fails leaves the window empty until
  a later batch retries it, as installs already do.
- `UnknownAttachment`: the window's session is gone. The window clears its
  attachment and is removed without a notice.
- Another assessment failure raises one notice and is not retried; the
  window stays, and a user close still works. A teardown error after the
  runtime accepted the close raises one notice and still removes the window.

### Removing a window never abandons Quit

Because an emptied close runs only while the pipeline is idle, a window
showing, assessing, or committing an Application close stays open even when
its session closes underneath it. The Application ticket covers every
terminal, so the session's removal makes its commit go stale and the Quit
assesses again. A confirmed Quit tears everything down. A cancelled Quit
clears `quitting` and the native request through `cancel_close`, as today,
and only then does the window take its emptied close.

### Confirm acts only on the confirmation it was drawn for

The Confirm button's listener captures the target and `CloseState::generation`
at render. A new `confirm_confirmation` method acts only while
`close.confirmation` still equals that target at that generation, the same
shape as `cancel_confirmation`. Every cancel and every new check bumps the
generation, so a stale click can reach neither a cancelled dialog nor a
replacement that asks the same question. `finish_close` keeps its current
contract, because the no-prompt path calls it with no confirmation showing.
The keyboard `dialog_confirm` already reads the current confirmation.

### Attachment events move to #26

The embedded close rules do not need them. The runtime resolves Detach
versus Session under its own lock, and a window learns that its session
closed from `SessionClosed`. In embedded mode only the owning window
attaches, detaches, or retargets its attachment, so no window needs to hear
about another's. A Detach never prompts, so no dialog has to explain that
other windows remain attached. Adding the events now would ship a reducer
change with no consumer.

[#26][issue-26] is the first consumer: a session picker that shows which
sessions other windows view. The shape to give it, so it is not designed
twice: `AttachmentCreated { attachment, session }`,
`AttachmentRetargeted { attachment, session }`, and
`AttachmentDetached { attachment }`. `SessionClosed` implies the detachment
of the session's attachments, as `Mux::close_session` does today, and the
snapshot carries the attachment map that `HierarchySnapshot` already
captures. `HierarchyEvent` is `#[non_exhaustive]`, so adding them later
breaks no client.

### `close_on_exit` moves to the runtime in the server half of #21

In embedded mode every window reads one process-wide configuration, and a
reload only changes future exits (`Desktop::exit_closes` records the policy
in force when the shell exited). Windows cannot disagree, so the
client-side policy is correct here. Disagreement starts when separate client
processes share one runtime. Which policy wins is lifecycle policy, not
transport, so it belongs to the server-mode half of #21 and ships with #45,
which first makes separate clients possible. The server plan must keep one
constraint from #22: one actor assesses and commits an exit-close. If the
runtime closes exited tabs itself, a client's showing confirmation over the
same tab must go stale through the revision check rather than commit against
a missing tab.

### Coordination with the runtime robustness work

Another thread owns `crates/huterm-core/src/terminal.rs` for #209, #168,
and #211. This plan changes nothing there. Termination evidence depends on its
teardown: `close_session` and `shutdown` call `TerminalRuntime::shutdown`,
and the new PTY tests assert reaping through it. If #211 changes the
teardown order, these tests are regression evidence for it; neither change
blocks the other. No change to `mux.rs` termination paths is planned.

## Races

| Race | Handling | Evidence |
| --- | --- | --- |
| Two windows of one session close at once | Both tickets resolve to Detach. The first commit bumps the revision; the second gets `StaleClose`, assesses again, and now resolves to Session. It prompts if a job runs. The session terminates once. | Core: `concurrent_window_tickets_reassess_when_last_attachment_changes`. New tier 2 test. |
| A close while a move changes the session | Moves bump the revision, so the ticket goes stale and the retry assesses the new scope. Moves are one Mux operation and one hierarchy event, so no client sees an intermediate empty state. The desktop has no move trigger until #26. | Core: `structural_changes_invalidate_consent_without_consuming_attachment`, extended with a cross-session `move_tab`. |
| A tab arrives in an emptied window's workspace | `EmptiedWindow` fails with `ViewNotEmpty` before the commit, or goes stale at it; the window stays and installs the tab. | New core and tier 2 tests. |
| A window closes its last tab while another tab is still installing | The follow-up is an emptied close, so Mux sees the installing tab and refuses it. | New tier 2 test. |
| The last tab moves to a sibling workspace of a sole-attachment session | `EmptiedWindow` detaches; the moved terminal survives in an unviewed session. | New core and tier 2 tests. |
| An emptied window's session closes during its assessment | `UnknownAttachment` removes the window silently. | New tier 2 test. |
| A window's session closes while it hosts Quit | The emptied close waits for the idle pipeline; the Quit goes stale and reassesses. | New tier 2 tests for confirming and committing. |
| A stale Confirm after Cancel | The generation guard ignores it: no assessment and no commit. | New tier 1 and tier 2 tests. |
| A stale Cancel after Confirm | Already guarded by `can_cancel_confirmation`. | `a_stale_cancel_after_confirm_cannot_reach_the_commit`. |
| Job evidence changes after consent | `covered_by` and the two-second age limit reject the commit, and the desktop prompts again only if the evidence widened. | Core: `new_background_job_requires_renewed_consent_and_quit_includes_zero_views`, `consent_survives_child_replacement_inside_the_same_live_job_group`. |
| Quit while a window close is in flight | The pending Application target is kept and runs after the window close (`quit_after`). Quit waits for pending spawns. | Existing tier 1 tests. |
| An exit-close against a window close | One actor per exit-close; a merged close that does not reach the tab returns it to owed. | Existing tier 2 exit-close tests. |

## Protocol and hierarchy changes

No `huterm-protocol` change: no new `HierarchyEvent`. Attachment events are
deferred to #26 with the shape above.

`huterm-core` gains `CloseRequest::EmptiedWindow { attachment, workspace }`
and `MuxError::ViewNotEmpty(WorkspaceId)`. Both are in-process Mux APIs. The
future IPC contract in #44 carries close requests, so it inherits the
variant.

## Implementation

Decided on 2026-10-25: one PR delivers everything below. Release Please's
commit override can split its changelog entry into several user-facing items
after merge.

### Stale Confirm guard

- Add `CloseState::can_confirm(target, generation)` and
  `WorkspaceView::confirm_confirmation`; capture the generation in the
  Confirm listener at render.

### Emptied windows

- Core: `CloseRequest::EmptiedWindow` and `MuxError::ViewNotEmpty` in
  `mux/lifecycle.rs` and `mux.rs`, resolved as above.
- Desktop: `CloseTarget::Emptied` in `CloseState`, offered through
  `next_request` after queued closes; `merge_close` lets Window and
  Application absorb it.
- Request it on the three edges above; `remove_closed_tabs` requests it
  instead of `CloseTarget::Window`. Keep the window marked emptied after
  `ViewNotEmpty` and request it again as the outcomes describe.
- Handle the outcomes above in `request_close` and `finish_close`.
- Cancel a tab confirmation whose tabs all left the window.
- Files: `desktop/windows.rs` (`CloseState`, `WorkspaceView` reconcile and
  close paths), `desktop/windows/projection.rs` (report removal of the last
  view), `desktop/windows/model.rs` if the model needs an emptied flag.

### Evidence and docs

- The core PTY tests, the tier 1 and tier 2 desktop tests, and the native
  smoke steps under Verification.
- The documentation changes under Documentation.

## Verification

Mapped to #21's acceptance criteria. "Existing" means the evidence is on
`main` and is cited, not rewritten.

### Embedded lifecycle verified independently; hidden quake keeps attachment

- Existing: `smoke:macos-quake` and `smoke:linux-quake` keep PTYs across
  hide and show, refuse hide while a confirmation shows, and show a hidden
  window's Quit confirmation.
- #41 is closed, so this criterion is already met in practice. This plan
  adds the rest of the embedded evidence below.

### Detach and switch keep children; termination spares siblings

- Existing: `detach_keeps_real_child_alive_and_last_window_close_reaps_it`.
- New core test: an attachment retargets from session A to session B while
  A runs a real child. A's child still answers input. `close_session(B)`
  reaps B's child (`kill(pid, 0)` returns `ESRCH`), and A's child still
  answers afterwards. This proves switch survival and sibling isolation in
  one behavior.
- Extended: `structural_changes_invalidate_consent_without_consuming_attachment`
  gains a cross-session `move_tab` between assessment and commit.
- New core test for `EmptiedWindow` resolution, one behavior with each
  case asserted: a workspace with tabs refuses with `ViewNotEmpty` and
  changes nothing; another attachment gives Detach; a sole attachment whose
  session keeps a live child in a sibling workspace gives Detach, and that
  child still answers afterwards; an empty session is deleted; an
  invalidated attachment fails with `UnknownAttachment`; a tab opened
  between prepare and commit makes the commit `StaleClose`; no resolution
  needs confirmation.
- Synchronize on shell ACK output through the test client, as the existing
  lifecycle tests do, never on sleeps.

### Two viewers, races, confirmations, jobs, and Quit versus close

Tier 2 (`desktop/windows/window_tests.rs`), with two windows attached to the
harness session. `DesktopHarness::send_line` and `canonical` create their
own attachments, so tests about the last attachment detach that observer
first or drive input through the windows.

- Closing one of two windows detaches it with no prompt, even while a job
  runs. The other window keeps its tabs and its shell answers.
- Concurrent closes: hold `Stage::Assess` and `Stage::Commit`, request
  `close_window` in both windows, release assessments, and wait until both
  commits are held. Releasing them, one commit returns `StaleClose` and
  assesses again (three assessments in all), the session terminates once,
  and neither window shows a close-failure notice. With a running job, the
  surviving request prompts after its retry.
- One window closes the shared workspace's last tab. The other window
  closes, the session is gone, and no notice appears.
- The harness closes the session through Mux, as another client would.
  Both windows close without a notice.
- With `Stage::Assess` held on an emptied close, the harness opens a tab in
  the workspace. After release the window stays, installs the tab, and the
  tab's child is alive.
- With `Stage::Subscribe` held so a newly opened tab is still installing,
  the window closes its last other tab. The window stays, the install
  completes, and the installing tab's child is alive. This fails on `main`,
  where the follow-up is a full `Window` close. A second case closes the
  installing tab through Mux before releasing `Stage::Subscribe`: the
  window then closes, so a refused emptied close is not lost.
- With the window as the session's sole attachment, the harness moves its
  last tab to a sibling workspace. The window closes by Detach, the session
  survives with zero attachments, and the moved tab's child is alive.
- Quit hosting: a window shows a Quit confirmation when the harness closes
  its session. The confirmation stays; Cancel clears `quitting`, and then
  the window closes. Separately, with `Stage::Commit` held on a confirmed
  Quit, closing the session does not lose the Quit: after release the
  application quits.
- An emptied close whose assessment fails (`WorkerHooks::fail`) raises one
  notice and is not retried; the window stays.
- The stale Confirm guard: after Cancel, a click through the Confirm
  listener starts no assessment and no commit. The test first observes that
  the listener ran, then runs the executor until parked before reading the
  `worker_hooks` counts. A current-generation click through the same
  listener commits exactly once, so a guard that always refuses fails.

Tier 1 (`desktop/windows/tests.rs`):

- `CloseState` refuses a stale Confirm: show, cancel, and confirm with the
  old generation; then show again with the same target and confirm with the
  old generation. The current generation is accepted.
- `merge_close` lets Window and Application absorb `Emptied`, and
  `next_request` offers `Emptied` only after queued user closes and while
  no confirmation shows.
- A session terminated by a window close is absent from a later Quit
  capture, while an unviewed session is present. This distinguishes Quit
  from last-window close.

Existing and cited: idle shells, foreground jobs, background groups,
orphans, and unknown state in `mux/lifecycle/tests.rs`; Quit capture and
rejected Quit in `desktop/windows/tests.rs`.

Each new test must fail at its assertion before its fix lands. For the two
fixes that comes from writing the test first. For tests over existing
behavior, one perturbation (skipping the retarget's preservation, or closing
both sessions) confirms the sibling test can fail.

### Native window, menu, and application-close paths

The menu items and shortcuts dispatch the same catalog invocations the
smokes already use, and `smoke:macos-menus` checks their native shortcuts.
The native close button and AppKit terminate have their own entry points,
so they get smoke steps in `scripts/check-viewers.ts`, which runs in both
palette smokes:

- Step 7 closes window 1 through the native path instead of `invoke-in`:
  `performClose:` on its `NSWindow` on macOS, and a `WM_DELETE_WINDOW`
  client message on X11, sent by a new `native-close\t<index>` fixture
  command. It still asserts that window 0's shell answers.
- On macOS, a new `native-terminate` fixture command calls
  `[NSApp terminate:nil]` while window 0 runs a live job. The viewers
  fixture shell gains a line that starts a foreground job and a line that
  ends it, since its loop runs no arbitrary commands. The check waits for
  the confirmation, cancels, ends the job, and requires a shell ACK.
- A new final step attaches a fresh window 1, closes the last tab there,
  and asserts that both windows close, neither reports a close failure, and
  the app exits with code 0 on its own. This holds only while no quake
  hotkey is registered, which the step checks through smoke state.

Both fixture commands run their AppKit or X11 call outside any GPUI
window update, deferred as the native Quit bridge already requires. The X11
client message needs the window's XID from GPUI's raw window handle and an
`x11rb` connection, which `huterm-gpui` already depends on. If that proves
unreliable under Xvfb, the Linux step falls back to the catalog path, and
the PR reports the native X11 close path as unverified.

Run `smoke:macos-palette` on the host. Check `df -h` before
`mise run linux:smoke`. Never run `vm:macos:smoke`.

### Commands

- Focused `cargo test -p huterm-core mux::lifecycle` and
  `cargo test -p huterm-gpui --features test-support windows::` while
  iterating, confirming the new tests appear in the runner's output.
- `mise run check` early and `mise run verify` before each handoff.
- `mise run test:ghostty-releasesafe`, because the PR adds `huterm-core`
  PTY tests.

### Residual risk

- Process creation after the final job snapshot remains an unavoidable race,
  as the README states.
- Physical WM behavior on GNOME or KDE close buttons is not automated; the
  X11 step covers the protocol GPUI receives.

## Documentation

- README: rewrite the window-lifecycle paragraph to match the table, add
  the emptied-window rule and the quake keep-alive exception, replace the
  `ps` description with `huterm-procinfo`, and keep the detection limits.
- `docs/agents/desktop-client.md`, Close and Quit: the emptied-window edges,
  the idle-pipeline rule for automatic closes, and the Confirm generation
  guard, each next to the related rule.
- `docs/agents/core-runtime.md`, Attachments and close consent: the
  `EmptiedWindow` resolution order.
- This plan: `Status: implemented` in the PR, with deviations recorded.

## Settled decisions

Agreed with the maintainer on 2026-10-25:

- An externally emptied window closes; retargeting it waits for #26.
- Attachment events move to #26, with the event shape recorded above.
- Runtime-owned `close_on_exit` belongs to #21's server half, delivered
  with #45.
- One PR, not three.
- The app keeps running with no windows while unviewed sessions survive,
  as `maybe_exit` does today. Dock reopen opens a fresh session and Linux
  keeps a windowless process; #26 decides how such sessions are reached.

## Open questions

None.

[issue-9]: https://github.com/jimeh/huterm/issues/9
[issue-20]: https://github.com/jimeh/huterm/issues/20
[issue-21]: https://github.com/jimeh/huterm/issues/21
[issue-22]: https://github.com/jimeh/huterm/issues/22
[issue-26]: https://github.com/jimeh/huterm/issues/26
[issue-41]: https://github.com/jimeh/huterm/issues/41
[issue-42]: https://github.com/jimeh/huterm/issues/42
[issue-45]: https://github.com/jimeh/huterm/issues/45
[issue-97]: https://github.com/jimeh/huterm/issues/97
[issue-98]: https://github.com/jimeh/huterm/issues/98
