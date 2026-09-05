# Huterm

Huterm combines a desktop terminal and a multiplexer. GUI and future TUI clients
provide independent views into the same workspaces.

## Language

**Workspace**:
A named collection of ordered tabs that exists independently of its views.
_Avoid_: Session, tab group.

**Tab**:
An ordered container within a workspace, containing one pane layout.

**Pane**:
A leaf in a tab's split tree that displays a terminal.

**Terminal**:
A PTY, its child process, terminal-emulator state, and scrollback.

**Client**:
A desktop, TUI, or command-line connection to the runtime.

**Window**:
A desktop view with its own selected workspace. A workspace can appear in more
than one window, and a window can switch between workspaces.

**Attachment**:
A view's connection to a workspace, with independent navigation and viewport
state. Detaching does not mean closing the workspace or its terminals.

**Client layout**:
The saved arrangement of a client's views, workspace selections, and presentation
preferences.

**Restoration**:
Recreating saved workspace structure and client layout after a restart, with
fresh terminal processes when the previous runtime is no longer alive.

**Reconnection**:
Attaching to a surviving runtime and its existing terminal processes.
