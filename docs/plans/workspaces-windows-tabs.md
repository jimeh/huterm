# Workspaces, windows, and tabs

Status: agreed product direction and first PR scope. No implementation is
claimed by this document. Terminology is defined in [CONTEXT.md](../../CONTEXT.md).

## Product direction

Huterm should replace both a desktop terminal such as Ghostty or iTerm and a
multiplexer such as tmux or herdr, with GUI and TUI clients eventually sharing
the same runtime.

A window can create and switch between workspaces, much like browser
workspaces. Switching one window must not change another window's selection.
Multiple windows or clients may eventually view the same workspace. A new
window and a new workspace are distinct operations, even if the first release
creates them together.

The runtime owns workspaces, ordered tabs, pane split trees, terminals, and
terminal metadata. Each view owns workspace selection, active tab and pane,
scroll positions, selection, and tab-bar placement. Each view remembers its
navigation state per workspace. Shared tab or pane mutations affect all views;
navigation affects only the initiating view.

Use Workspace in both product language and the core model. Existing SessionId
and session terminology describe the earlier proof of concept and should migrate
with the ownership implementation. Historical plans need not be rewritten.

## First PR: multiple windows and tabs

Deliver independent native windows with working tabs on macOS and Linux. Each
window initially attaches to a fresh workspace; workspace switching and shared
attachments remain follow-up work.

- Provide New Window, New Tab, Close Tab, next/previous tab, and direct tab
  selection through appropriate menus, keyboard actions, and tab controls.
- Support top, bottom, left, and right tab placement through configuration.
  Placement determines orientation; do not introduce conflicting settings.
- Use one tab model with horizontal and vertical presentations. Handle overflow
  and narrow windows, and keep the active tab visible.
- Start with application-provided terminal titles, falling back to the launched
  program name. Do not interpret title text as directory or process metadata.
- Preserve terminal processes and per-tab view state when switching tabs.
  Retain the existing rule that changed terminal contents invalidate selection.
- Continue processing output in hidden tabs, but avoid hidden-tab snapshot
  preparation and painting. Continue consuming lifecycle and title events.
- Apply configuration reload across open windows and retained tabs, including
  geometry invalidation when placement changes.
- Keep existing initial-directory behavior for this PR. Live directory labels,
  process discovery, and directory inheritance follow separately.

Agreed initial close policy: closing a tab terminates its terminal;
closing a window explicitly closes that window's private workspace. Closing
the last tab closes the window, and closing the last window exits the app,
matching the current application lifetime. Keep this policy in desktop commands,
not view destructors or attachment removal. Shared attachments and a server
will require revisiting window-close policy without changing core detach
semantics. Child exit remains a visible exited tab until explicitly closed.
Ask for confirmation before closing tabs or windows with foreground jobs.
Use top placement by default and standard platform shortcuts, verified through
the real keybinding matcher.

## Implementation sequence and boundaries

1. Expand core's ownership map into workspaces with ordered tabs and one pane
   per tab. Allocate IDs centrally and preserve identity independently of
   collection indices and window handles. Leave room for restoring IDs and
   advancing allocation without collisions; serialization is not needed yet.
2. Add core operations for creating and closing workspaces and tabs, with one
   owner ordering structural mutations. Publish a tab only after successful
   terminal creation, or expose an explicit pending state. Failed creation must
   not leak workers or leave a phantom tab. Keep PTY spawn and shutdown waits
   off the GPUI thread.
3. Separate application runtime ownership, window attachment state, and
   terminal views. Dropping or replacing a view must not implicitly close its
   terminal. Explicit close stops and joins the affected runtime; application
   quit cleans up all remaining runtimes.
4. Add window commands and tab navigation, retaining each tab's view state.
   Keep metadata/lifecycle consumption separate from visible rendering work.
   Avoid multiplying the current 16 ms snapshot refresh behavior across hidden
   terminals.
5. Add all four tab placements. Compute terminal bounds after subtracting
   titlebar and tab chrome, then use the same bounds for painting, input,
   selection, scrollbars, and PTY sizing. Hidden terminals retain their last
   size and resize before presentation when activated.

Keep typed in-process operations. Do not add a transport framework, server,
serialization, or a split UI merely to anticipate them. Runtime-owned tabs
avoid moving canonical state out of GPUI later. Independent attachments avoid
coupling workspace selection across windows.

## Follow-up features

### Terminal metadata and directory inheritance

Support directory, foreground process, or combined tab labels. Horizontal tabs
can use compact labels; vertical tabs can use two lines and a resizable sidebar.
Expose full paths on hover. Start with display options rather than a template
language.

Keep title, shell working directory, foreground job, and remote host distinct.
Use shell directory reporting such as OSC 7 and bounded local process inspection
off the UI thread. OSC 7 includes a host and path; see the
[protocol documentation](https://iterm2.com/3.5/documentation-escape-codes.html).
Verify the pinned parser's support before choosing an integration mechanism.
Process and remote command labels may be best-effort without shell integration.

New-tab policy should support home or inheritance from the initiating view's
active pane, with inheritance recommended as the default. Resolve the source
when the command is invoked, not after a later tab switch. Prefer the shell's
directory over an editor's internal directory. Fall back to home with a brief
explanation when no usable local directory is available; never treat a reported
remote path as a local launch directory. Keep new-window policy separate.

### Workspace switching and pane splits

Add create/select workspace within an existing window, then additional views
of a workspace. Switching must retain inactive workspaces and restore each
view's navigation state. Tabs own split trees; metadata and new-terminal
directory selection follow the active pane. Moving a tab between workspaces
is a structural transfer, distinct from attaching another view.

### Restoration

Persist versioned workspace records containing IDs, names, tab order, split
geometry, and shell launch directories. Persist client layout separately,
including window geometry, workspace selections, per-workspace active tab/pane,
and tab presentation preferences. One client's layout must not overwrite another
client's presentation choices.

Restore structure and fresh shells after a full runtime restart. This does not
restore arbitrary running processes or promise scrollback recovery. Reconnection
to a surviving server instead retains live processes and scrollback. Design
atomic saves, schema migration, missing-directory fallback, and partial restore
failure handling when implementing persistence. Saved process labels must not
be interpreted as commands to execute.

### Server and multiple clients

Move existing runtime ownership behind a versioned local transport, then add
the TUI client. Detach removes an attachment; explicit terminal close and
workspace deletion terminate shared processes. Decide last-attachment and
window-close behavior before exposing shared workspaces.

Keep viewport dimensions per attachment. Before simultaneous clients ship,
choose an explicit terminal-size arbitration policy, distribute structural and
metadata updates to each attachment, and define stale-operation errors. Cloning
an event receiver is not a substitute for broadcasting updates to clients.

## Verification

During implementation, add focused core tests for ownership, tab ordering,
stable identity, failed creation, explicit close, and detach without shutdown.
Use PTY integration tests to prove closing one tab/window terminates its workers
and children without affecting siblings. Verify background output and child
exit remain observable while tabs are hidden.

Exercise real action matching and input routing so new shortcuts do not reach
the shell. Check tab switches preserve scroll position and selection under the
existing invalidation rules. Test layout calculations for all placements,
overflow, small windows, and titlebar/fullscreen changes.

Run native macOS and Linux UI checks for multiple windows, focus, all tab
placements, resize, configuration reload, and close/quit. Confirm hidden tabs
do not generate viewport snapshots, and run the existing scroll benchmark to
check visible-terminal regressions. Run `mise run verify` before implementation
handoff; a green unit suite alone does not prove native focus or geometry.

## Open decisions

- When workspace switching ships, whether New Window shows the current
  workspace, a picker, or a fresh workspace.
- Simultaneous-client size arbitration and server lifetime policy.
- Persistence format and location, and the desired extent of scrollback recovery.

These decisions do not block the first PR. The initial close policy above is
specific to private backing workspaces, not a permanent rule for server clients.
