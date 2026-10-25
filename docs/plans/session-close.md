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
   core has invalidated.
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
| Close a window's last tab | Same | As above, then a window close | Window close, below | For the tab; the window close then covers no terminal |
| Close a window, another attachment holds the session | `close_window`, title-bar button, WM close, menu | `detach_session`: viewers revoked, terminals untouched | Window removed | Never: no terminal is affected |
| Close a window, last attachment | Same | `close_session`: every terminal stopped and reaped | Window removed; the app exits when no window and no session remain | Yes, across every terminal in the session |
| Root shell exits, `close_on_exit = true` | Shell exit | One window commits `close_tab` | Tab removed; last tab closes the window | Never |
| Root shell exits, `close_on_exit = false` | Shell exit | None; the child is reaped, history kept | Exited tab retained | Not applicable |
| Window emptied by another client (new) | Its tabs closed or moved elsewhere | Window close, committed only if it stops no terminal | Window removed | Never |
| Window's session closed by another client (new) | `SessionClosed` | None; core already invalidated the attachment | Window removed without a Mux call | Never |
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

## Decisions

### An emptied window closes

When reconcile removes a window's last tab view, the window closes as if
its own last tab had closed: it sends `CloseRequest::Window`. If another
attachment holds the session, that detaches; otherwise it terminates the
now-empty session. This matches the local last-tab rule and tmux's default
`detach-on-destroy on`. Keeping an empty window open would leave a view with
nothing to show and no way to choose a session until #26. Retargeting to
another session belongs with the session switcher in #26, which may revisit
this rule.

The trigger is the edge, not the state. A window counts as emptied only
when a reconcile removal took its last view. New windows before their first
spawn, `attach-window` fixture windows before their installs, and the no-PTY
Quit host have no views to remove, so they never trigger it.

The emptied close must never stop a terminal. Between the projection
showing an empty window and the assessment, a tab may arrive in the session,
for example through a later move. The window therefore commits an emptied
close only when the assessment covers no terminal
(`CloseAssessment::terminals()` is empty). Otherwise it drops the close, and
reconcile installs the arriving tab. The commit's revision check covers
changes after the assessment: any `open_tab` or move bumps the revision, the
commit returns `StaleClose`, and the retry checks emptiness again before it
assesses. This needs no core change. A dedicated `CloseRequest::EmptiedWindow`
was considered and rejected: the existing assessment and revision check give
the same guarantee without widening the API.

The emptied close is an automatic request, like an exit-close. It waits
behind queued user closes and a showing confirmation in `next_request`, and
it is never merged into a user's confirmation. A tab confirmation whose tabs
all left the window is cancelled, since its commit could only go stale.

### A window whose session closed is removed locally

When the projection no longer holds the window's attached session, core has
already invalidated the attachment. The window clears its attachment in the
model and removes itself without assessing, so it raises no "Cannot assess
close" notice. If an emptied close was already assessing when the session
closed, an `UnknownAttachment` result takes the same silent path. Any
confirmation the window showed is dropped with it.

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
| A tab arrives in an emptied window's session | The emptied close commits only when it stops no terminal; otherwise it is dropped and the tab installs. | New tier 2 test. |
| An emptied window's session closes during its assessment | `UnknownAttachment` removes the window silently. | New tier 2 test. |
| A stale Confirm after Cancel | The generation guard ignores it: no assessment and no commit. | New tier 1 and tier 2 tests. |
| A stale Cancel after Confirm | Already guarded by `can_cancel_confirmation`. | `a_stale_cancel_after_confirm_cannot_reach_the_commit`. |
| Job evidence changes after consent | `covered_by` and the two-second age limit reject the commit, and the desktop prompts again only if the evidence widened. | Core: `new_background_job_requires_renewed_consent_and_quit_includes_zero_views`, `consent_survives_child_replacement_inside_the_same_live_job_group`. |
| Quit while a window close is in flight | The pending Application target is kept and runs after the window close (`quit_after`). Quit waits for pending spawns. | Existing tier 1 tests. |
| An exit-close against a window close | One actor per exit-close; a merged close that does not reach the tab returns it to owed. | Existing tier 2 exit-close tests. |

## Protocol and hierarchy changes

None. No new `HierarchyEvent`, `CloseRequest`, or `MuxError` variant.
Attachment events are deferred to #26 with the shape above.

## Implementation

Decided on 2026-10-25: one PR delivers everything below. Release Please's
commit override can split its changelog entry into several user-facing items
after merge.

### Stale Confirm guard

- Add `CloseState::can_confirm(target, generation)` and
  `WorkspaceView::confirm_confirmation`; capture the generation in the
  Confirm listener at render.

### Emptied windows

- Mark a window emptied when a reconcile removal takes its last view; offer
  the emptied close through `next_request` after queued closes and exit
  closes.
- Commit the emptied close only for an assessment with no terminals; drop it
  otherwise.
- Remove a window locally when its attached session left the projection,
  and handle `UnknownAttachment` from an emptied close the same way.
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
- Synchronize on shell ACK output through the test client, as the existing
  lifecycle tests do, never on sleeps.

### Two viewers, races, confirmations, jobs, and Quit versus close

Tier 2 (`desktop/windows/window_tests.rs`), with two windows attached to the
harness session:

- Closing one of two windows detaches it with no prompt, even while a job
  runs. The other window keeps its tabs and its shell answers.
- Both windows request `close_window` while `Stage::Assess` is held. After
  release, the second commit is refused and assessed again, the session
  terminates once, and neither window shows a close-failure notice.
  With a running job, the surviving request prompts after its retry.
- One window closes the shared workspace's last tab. The other window
  closes, the session is gone, and no notice appears.
- The harness closes the session through Mux, as another client would.
  Both windows close without a Mux call failing and without a notice.
- With `Stage::Assess` held on an emptied close, the harness opens a tab in
  the session. After release the window stays, installs the tab, and the
  tab's child is alive.
- The stale Confirm guard: after Cancel, a click through the Confirm
  listener starts no assessment and no commit (`worker_hooks` counts).

Tier 1 (`desktop/windows/tests.rs`):

- `CloseState` refuses a stale Confirm: show, cancel, and confirm with the
  old generation; then show again with the same target and confirm with the
  old generation.
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
smokes already use, and `smoke:macos-menus` checks their native shortcuts. The
native close button and AppKit terminate have their own entry points, so
they get smoke steps in `scripts/check-viewers.ts`, which runs in both
palette smokes:

- Step 7 closes window 1 through the native path instead of `invoke-in`:
  `performClose:` on its `NSWindow` on macOS, and a `WM_DELETE_WINDOW`
  client message on X11, sent by a new `native-close\t<index>` fixture
  command. It still asserts that window 0's shell answers.
- On macOS, a new `native-terminate` fixture command calls
  `[NSApp terminate:nil]` with a live job in window 0. The check waits for
  the confirmation, cancels, and requires a shell ACK afterwards.
- A new final step attaches a fresh window 1, closes the last tab there,
  and asserts that both windows close, neither reports a close failure, and
  the app exits with code 0 on its own (no window and no session left).

The X11 client message needs the window's XID from GPUI's raw window handle
and an `x11rb` connection, which `huterm-gpui` already depends on. If that
proves unreliable under Xvfb, fall back to the catalog path on Linux and say
so in the PR.

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

- README: rewrite the window-lifecycle paragraph to match the table, add the
  emptied-window rule, replace the `ps` description with `huterm-procinfo`,
  and keep the detection limits.
- `docs/agents/desktop-client.md`, Close and Quit: the emptied-window edge
  trigger, the no-terminal commit rule, and the Confirm generation guard,
  each next to the related rule.
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
