//! The request inventory: what a client asks of a server, and the wire
//! mirrors of the core types requests carry.
//!
//! Every request has a stable tag, a [`RequestKind`] with a static
//! [`RetryClass`], and the minor version that introduced it, so a client
//! refuses locally a request the connection did not negotiate. The
//! inventory mirrors the operations the desktop reaches through core's
//! `Mux` and catalog commands. `huterm-wire` cannot depend on core, so the
//! close, move, and restore types here are minimal mirrors a server
//! converts.
//!
//! Requests that name a terminal viewer travel on that viewer's ordering
//! lane, so they stay in order with its input; [`RequestKind::names_viewer`]
//! says which.

use std::num::NonZeroU64;

use huterm_protocol::{
    Arrangement, AttachmentId, BufferPoint, BufferRange, CellSize, GridSize,
    MAX_LAYOUT_DEPTH, MAX_NAME_BYTES, MAX_SPLIT_CHILDREN, MousePosition,
    NodeId, PaneId, PaneLayout, ScrollCommand, ScrollbackLimit, SelectionUnit,
    SessionId, SplitAxis, SplitDirection, SplitId, TabId, TerminalCommand,
    TerminalId, TerminalPresentation, ViewerId, WorkspaceId,
};

use crate::codec::{DecodeError, EncodeError, IdScope, Reader, Writer};
use crate::command::{
    DecodedInvocation, decode_invocation, encode_decoded_invocation,
};
use crate::hierarchy::{Items, decode_layout_counted, encode_layout};
use crate::launch::{
    decode_cell_size, decode_command, decode_grid, decode_presentation,
    decode_scrollback, encode_cell_size, encode_command, encode_grid,
    encode_presentation, encode_scrollback,
};
use crate::viewer::{
    SubscribeOptions, decode_subscribe_options, encode_subscribe_options,
    read_point, read_position, read_range, read_scroll, read_unit, write_point,
    write_position, write_range, write_scroll, write_unit,
};

/// The longest name a rename or create request carries. It is above
/// [`MAX_NAME_BYTES`], so core, not the codec, refuses a name between the
/// two with `InvalidName`, as it does in process; a sender refuses a longer
/// one locally.
pub const MAX_REQUEST_NAME_BYTES: usize = 4 * MAX_NAME_BYTES;

/// The most panes and splits of one layout, and the most nodes of one
/// arrangement, a request carries. A sender refuses a larger one locally.
pub const MAX_REQUEST_LAYOUT_ITEMS: usize = 1024;

/// The most tabs one close request names.
pub const MAX_CLOSE_REQUEST_TABS: usize = 1024;

/// A request's identity on its connection: assigned by the client from 1,
/// never reused, and never 0.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RequestId(NonZeroU64);

impl RequestId {
    /// A connection's first request.
    pub const FIRST: Self = Self(NonZeroU64::MIN);

    /// The identity `value`, or `None` for 0.
    #[must_use]
    pub const fn new(value: u64) -> Option<Self> {
        match NonZeroU64::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    /// The identity's number.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }

    /// The identity after this one, or `None` once numbers run out.
    #[must_use]
    pub const fn next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }
}

pub(crate) fn write_request_id(w: &mut Writer, id: RequestId) {
    w.u64(id.get());
}

pub(crate) fn read_request_id(
    r: &mut Reader<'_>,
) -> Result<RequestId, DecodeError> {
    RequestId::new(r.u64()?).ok_or(DecodeError::Invalid("request ID"))
}

/// What a client does with a request in flight when its connection breaks.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RetryClass {
    /// A runtime query: sent again on the next connection.
    Query,
    /// A request that names a viewer: fails with `Disconnected`, because
    /// the viewer ended with the connection.
    Viewer,
    /// A change that may have committed: never sent again, and resolves
    /// `OutcomeUnknown`.
    Mutation,
}

/// Which request a [`Request`] is. Its number is the request's stable tag.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub enum RequestKind {
    /// [`Request::SubscribeHierarchy`].
    SubscribeHierarchy,
    /// [`Request::ResyncHierarchy`].
    ResyncHierarchy,
    /// [`Request::SelectSession`].
    SelectSession,
    /// [`Request::SelectWorkspace`].
    SelectWorkspace,
    /// [`Request::SelectTab`].
    SelectTab,
    /// [`Request::SelectPane`].
    SelectPane,
    /// [`Request::PrepareClose`].
    PrepareClose,
    /// [`Request::CreateSession`].
    CreateSession,
    /// [`Request::CreateWorkspace`].
    CreateWorkspace,
    /// [`Request::CreateWorkspaceWithTab`].
    CreateWorkspaceWithTab,
    /// [`Request::OpenTab`].
    OpenTab,
    /// [`Request::SplitPane`].
    SplitPane,
    /// [`Request::ClosePane`].
    ClosePane,
    /// [`Request::CloseTab`].
    CloseTab,
    /// [`Request::CloseWorkspace`].
    CloseWorkspace,
    /// [`Request::CloseSession`].
    CloseSession,
    /// [`Request::RenameSession`].
    RenameSession,
    /// [`Request::RenameWorkspace`].
    RenameWorkspace,
    /// [`Request::RenameTab`].
    RenameTab,
    /// [`Request::RenamePane`].
    RenamePane,
    /// [`Request::SetWorkspacePinned`].
    SetWorkspacePinned,
    /// [`Request::ToggleInputSync`].
    ToggleInputSync,
    /// [`Request::CloseEmptiedWorkspace`].
    CloseEmptiedWorkspace,
    /// [`Request::ResizeSplit`].
    ResizeSplit,
    /// [`Request::SetPaneZoom`].
    SetPaneZoom,
    /// [`Request::ReorderTab`].
    ReorderTab,
    /// [`Request::MoveTab`].
    MoveTab,
    /// [`Request::MoveSession`].
    MoveSession,
    /// [`Request::MoveWorkspace`].
    MoveWorkspace,
    /// [`Request::MoveWorkspaceToNewSession`].
    MoveWorkspaceToNewSession,
    /// [`Request::SwapPanes`].
    SwapPanes,
    /// [`Request::MovePane`].
    MovePane,
    /// [`Request::MoveTabToPane`].
    MoveTabToPane,
    /// [`Request::ArrangePanes`].
    ArrangePanes,
    /// [`Request::RestoreLayout`].
    RestoreLayout,
    /// [`Request::AttachSession`].
    AttachSession,
    /// [`Request::DetachSession`].
    DetachSession,
    /// [`Request::RetargetAttachment`].
    RetargetAttachment,
    /// [`Request::CommitClose`].
    CommitClose,
    /// [`Request::ReleaseCloseTicket`].
    ReleaseCloseTicket,
    /// [`Request::ExecuteCommand`].
    ExecuteCommand,
    /// [`Request::SetScrollback`].
    SetScrollback,
    /// [`Request::ClearHistory`].
    ClearHistory,
    /// [`Request::Reset`].
    Reset,
    /// [`Request::ReportGeometry`].
    ReportGeometry,
    /// [`Request::SubscribeTerminal`].
    SubscribeTerminal,
    /// [`Request::CloseViewer`].
    CloseViewer,
    /// [`Request::Snapshot`].
    Snapshot,
    /// [`Request::Selection`].
    Selection,
    /// [`Request::SelectUnits`].
    SelectUnits,
    /// [`Request::UpdatePresentation`].
    UpdatePresentation,
    /// [`Request::SetScrollbackCompression`].
    SetScrollbackCompression,
    /// `Request::PinTab`, which exists only at the test-only minor
    /// version.
    #[cfg(any(test, feature = "test-minor"))]
    PinTab,
}

impl RequestKind {
    /// Every kind this build defines, in number order.
    pub const ALL: &'static [Self] = &[
        Self::SubscribeHierarchy,
        Self::ResyncHierarchy,
        Self::SelectSession,
        Self::SelectWorkspace,
        Self::SelectTab,
        Self::SelectPane,
        Self::PrepareClose,
        Self::CreateSession,
        Self::CreateWorkspace,
        Self::CreateWorkspaceWithTab,
        Self::OpenTab,
        Self::SplitPane,
        Self::ClosePane,
        Self::CloseTab,
        Self::CloseWorkspace,
        Self::CloseSession,
        Self::RenameSession,
        Self::RenameWorkspace,
        Self::RenameTab,
        Self::RenamePane,
        Self::SetWorkspacePinned,
        Self::ToggleInputSync,
        Self::CloseEmptiedWorkspace,
        Self::ResizeSplit,
        Self::SetPaneZoom,
        Self::ReorderTab,
        Self::MoveTab,
        Self::MoveSession,
        Self::MoveWorkspace,
        Self::MoveWorkspaceToNewSession,
        Self::SwapPanes,
        Self::MovePane,
        Self::MoveTabToPane,
        Self::ArrangePanes,
        Self::RestoreLayout,
        Self::AttachSession,
        Self::DetachSession,
        Self::RetargetAttachment,
        Self::CommitClose,
        Self::ReleaseCloseTicket,
        Self::ExecuteCommand,
        Self::SetScrollback,
        Self::ClearHistory,
        Self::Reset,
        Self::ReportGeometry,
        #[cfg(any(test, feature = "test-minor"))]
        Self::PinTab,
        Self::SubscribeTerminal,
        Self::CloseViewer,
        Self::Snapshot,
        Self::Selection,
        Self::SelectUnits,
        Self::UpdatePresentation,
        Self::SetScrollbackCompression,
    ];

    /// The kind's stable number, the request's tag.
    #[must_use]
    pub const fn number(self) -> u64 {
        match self {
            Self::SubscribeHierarchy => 0,
            Self::ResyncHierarchy => 1,
            Self::SelectSession => 2,
            Self::SelectWorkspace => 3,
            Self::SelectTab => 4,
            Self::SelectPane => 5,
            Self::PrepareClose => 6,
            Self::CreateSession => 7,
            Self::CreateWorkspace => 8,
            Self::CreateWorkspaceWithTab => 9,
            Self::OpenTab => 10,
            Self::SplitPane => 11,
            Self::ClosePane => 12,
            Self::CloseTab => 13,
            Self::CloseWorkspace => 14,
            Self::CloseSession => 15,
            Self::RenameSession => 16,
            Self::RenameWorkspace => 17,
            Self::RenameTab => 18,
            Self::RenamePane => 19,
            Self::SetWorkspacePinned => 20,
            Self::ToggleInputSync => 21,
            Self::CloseEmptiedWorkspace => 22,
            Self::ResizeSplit => 23,
            Self::SetPaneZoom => 24,
            Self::ReorderTab => 25,
            Self::MoveTab => 26,
            Self::MoveSession => 27,
            Self::MoveWorkspace => 28,
            Self::MoveWorkspaceToNewSession => 29,
            Self::SwapPanes => 30,
            Self::MovePane => 31,
            Self::MoveTabToPane => 32,
            Self::ArrangePanes => 33,
            Self::RestoreLayout => 34,
            Self::AttachSession => 35,
            Self::DetachSession => 36,
            Self::RetargetAttachment => 37,
            Self::CommitClose => 38,
            Self::ReleaseCloseTicket => 39,
            Self::ExecuteCommand => 40,
            Self::SetScrollback => 41,
            Self::ClearHistory => 42,
            Self::Reset => 43,
            Self::ReportGeometry => 44,
            #[cfg(any(test, feature = "test-minor"))]
            Self::PinTab => 45,
            Self::SubscribeTerminal => 46,
            Self::CloseViewer => 47,
            Self::Snapshot => 48,
            Self::Selection => 49,
            Self::SelectUnits => 50,
            Self::UpdatePresentation => 51,
            Self::SetScrollbackCompression => 52,
        }
    }

    /// The kind with this number, if this build defines one.
    #[must_use]
    pub fn from_number(number: u64) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|kind| kind.number() == number)
    }

    /// What a client does with the request when its connection breaks.
    #[must_use]
    pub const fn retry_class(self) -> RetryClass {
        match self {
            Self::SubscribeHierarchy
            | Self::ResyncHierarchy
            | Self::SelectSession
            | Self::SelectWorkspace
            | Self::SelectTab
            | Self::SelectPane
            | Self::PrepareClose => RetryClass::Query,
            Self::ReportGeometry
            | Self::SubscribeTerminal
            | Self::CloseViewer
            | Self::Snapshot
            | Self::Selection
            | Self::SelectUnits
            | Self::UpdatePresentation => RetryClass::Viewer,
            _ => RetryClass::Mutation,
        }
    }

    /// The minor version that introduced the request. A client refuses a
    /// newer request locally with `Unsupported`.
    #[must_use]
    pub const fn since(self) -> u16 {
        match self {
            #[cfg(any(test, feature = "test-minor"))]
            Self::PinTab => crate::test_minor::TEST_MINOR,
            _ => 0,
        }
    }

    /// Whether the request's result may exceed the `Result` cap, so it
    /// travels as a `LargeResult` on the large-result lane. A connection
    /// holds at most one such request in flight. Selection text is the
    /// only one.
    #[must_use]
    pub const fn large_result(self) -> bool {
        matches!(self, Self::Selection)
    }

    /// Whether the request names a viewer, so it travels on that viewer's
    /// ordering lane and stays in order with the viewer's input.
    #[must_use]
    pub const fn names_viewer(self) -> bool {
        matches!(
            self,
            Self::SetScrollback
                | Self::ClearHistory
                | Self::Reset
                | Self::ReportGeometry
                | Self::CloseViewer
                | Self::Snapshot
                | Self::Selection
                | Self::SelectUnits
                | Self::UpdatePresentation
                | Self::SetScrollbackCompression
        )
    }
}

/// A close scope, as core's `CloseRequest` names it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CloseRequest {
    /// Close one view, ending its session only if it is the last view.
    Window(AttachmentId),
    /// End a session.
    Session(SessionId),
    /// End a workspace and its tabs.
    Workspace(WorkspaceId),
    /// Close a tab.
    Tab {
        /// Expected owning workspace.
        workspace: WorkspaceId,
        /// The tab.
        tab: TabId,
    },
    /// Close one pane of a tab.
    Pane {
        /// Expected owning workspace.
        workspace: WorkspaceId,
        /// Expected owning tab.
        tab: TabId,
        /// The pane.
        pane: PaneId,
    },
    /// Close several tabs of one workspace, at most
    /// [`MAX_CLOSE_REQUEST_TABS`].
    Tabs {
        /// Expected owning workspace of every tab.
        workspace: WorkspaceId,
        /// The tabs.
        tabs: Vec<TabId>,
    },
    /// End every session.
    Application,
    /// Close a window whose last view went away.
    EmptiedWindow {
        /// The window's attachment.
        attachment: AttachmentId,
        /// The workspace the window showed.
        workspace: WorkspaceId,
    },
}

/// The effect a close resolved to, as core's `CloseEffect` names it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CloseEffect {
    /// Only remove the attachment.
    Detach(AttachmentId),
    /// End the session.
    Session(SessionId),
    /// End the workspace.
    Workspace(WorkspaceId),
    /// Keep the attachment and move its window off the workspace.
    LeaveWorkspace {
        /// The window's attachment.
        attachment: AttachmentId,
        /// The workspace the window showed.
        workspace: WorkspaceId,
    },
    /// End one tab.
    Tab {
        /// Expected owning workspace.
        workspace: WorkspaceId,
        /// The tab.
        tab: TabId,
    },
    /// End one pane of a tab that keeps others.
    Pane {
        /// Expected owning workspace.
        workspace: WorkspaceId,
        /// Expected owning tab.
        tab: TabId,
        /// The pane.
        pane: PaneId,
    },
    /// End several tabs of one workspace.
    Tabs {
        /// Expected owning workspace.
        workspace: WorkspaceId,
        /// Distinct tabs in request order.
        tabs: Vec<TabId>,
    },
    /// End the whole runtime.
    Application,
}

/// A pane with the tab and workspace it is expected in, as core's
/// `PaneRef` names it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PaneRef {
    /// Expected workspace of the pane's tab.
    pub workspace: WorkspaceId,
    /// Expected tab of the pane.
    pub tab: TabId,
    /// The pane.
    pub pane: PaneId,
}

/// A side of an existing pane, as core's `Beside` names it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Beside {
    /// The target pane.
    pub target: PaneRef,
    /// The side of the target the moved node takes.
    pub side: SplitDirection,
}

/// Where a moved pane lands, as core's `MoveDestination` names it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MoveDestination {
    /// Beside a pane.
    Beside(Beside),
    /// Alone in a new tab of a workspace.
    NewTab {
        /// Destination workspace.
        workspace: WorkspaceId,
        /// The tab the new tab goes before; `None` appends.
        before: Option<TabId>,
    },
}

/// Which side of a recorded layout change a restore returns to.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RestoreDirection {
    /// Back to the change's `before`.
    Undo,
    /// Forward to the change's `after`.
    Redo,
}

/// One request. Its fields mirror the core call the server makes.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Request {
    /// Subscribe the connection to the hierarchy. The result carries the
    /// subscribed sequence and its digest; the snapshot travels first on
    /// the hierarchy lane.
    SubscribeHierarchy,
    /// Replace the connection's hierarchy feed with a fresh snapshot.
    ResyncHierarchy,
    /// Validate a session selection.
    SelectSession {
        /// The session.
        session: SessionId,
    },
    /// Resolve a workspace to its session.
    SelectWorkspace {
        /// The workspace.
        workspace: WorkspaceId,
    },
    /// Resolve a tab to its workspace and session.
    SelectTab {
        /// The tab.
        tab: TabId,
    },
    /// Resolve a pane to its tab, workspace, and session.
    SelectPane {
        /// The pane.
        pane: PaneId,
    },
    /// Resolve a close and assess its jobs; the result names a ticket the
    /// connection holds. It commits nothing, so it is a query.
    PrepareClose {
        /// The close.
        request: CloseRequest,
    },
    /// Create an empty session.
    CreateSession {
        /// The custom name, if any.
        name: Option<String>,
    },
    /// Append an empty workspace to a session.
    CreateWorkspace {
        /// The session.
        session: SessionId,
        /// The custom name, if any.
        name: Option<String>,
    },
    /// Append a workspace with one tab, as one step.
    CreateWorkspaceWithTab {
        /// The session.
        session: SessionId,
        /// The custom name, if any.
        name: Option<String>,
        /// The tab's terminal.
        command: Box<TerminalCommand>,
    },
    /// Append a tab to a workspace.
    OpenTab {
        /// The workspace.
        workspace: WorkspaceId,
        /// The tab's terminal.
        command: Box<TerminalCommand>,
    },
    /// Split a pane with a new terminal.
    SplitPane {
        /// Expected workspace.
        workspace: WorkspaceId,
        /// Expected tab.
        tab: TabId,
        /// The pane to split.
        pane: PaneId,
        /// The side the new pane takes.
        direction: SplitDirection,
        /// The new pane's terminal.
        command: Box<TerminalCommand>,
    },
    /// Close a pane.
    ClosePane {
        /// Expected workspace.
        workspace: WorkspaceId,
        /// Expected tab.
        tab: TabId,
        /// The pane.
        pane: PaneId,
    },
    /// Close a tab.
    CloseTab {
        /// Expected workspace.
        workspace: WorkspaceId,
        /// The tab.
        tab: TabId,
    },
    /// Close a workspace.
    CloseWorkspace {
        /// The workspace.
        workspace: WorkspaceId,
    },
    /// Close a session.
    CloseSession {
        /// The session.
        session: SessionId,
    },
    /// Set or clear a session's custom name.
    RenameSession {
        /// The session.
        session: SessionId,
        /// The name; `None` clears it.
        name: Option<String>,
    },
    /// Set or clear a workspace's custom name.
    RenameWorkspace {
        /// The workspace.
        workspace: WorkspaceId,
        /// The name; `None` clears it.
        name: Option<String>,
    },
    /// Set or clear a tab's custom name.
    RenameTab {
        /// The tab.
        tab: TabId,
        /// The name; `None` clears it.
        name: Option<String>,
    },
    /// Set or clear a pane's custom name.
    RenamePane {
        /// The pane.
        pane: PaneId,
        /// The name; `None` clears it.
        name: Option<String>,
    },
    /// Pin or unpin a workspace.
    SetWorkspacePinned {
        /// The workspace.
        workspace: WorkspaceId,
        /// Whether it is pinned.
        pinned: bool,
    },
    /// Turn a tab's input sync off when on and on when off, read and set
    /// under one lock, so two quick toggles cancel out.
    ToggleInputSync {
        /// Expected workspace.
        workspace: WorkspaceId,
        /// The tab.
        tab: TabId,
    },
    /// Close a workspace a window left empty when its session holds
    /// another; the result says whether it closed.
    CloseEmptiedWorkspace {
        /// The session.
        session: SessionId,
        /// The workspace.
        workspace: WorkspaceId,
    },
    /// Set the weights of a split's children.
    ResizeSplit {
        /// Expected workspace.
        workspace: WorkspaceId,
        /// Expected tab.
        tab: TabId,
        /// The split.
        split: SplitId,
        /// The split's children, which must be unchanged, at most
        /// [`MAX_SPLIT_CHILDREN`].
        children: Vec<NodeId>,
        /// Their new weights, at most [`MAX_SPLIT_CHILDREN`].
        weights: Vec<u32>,
    },
    /// Set or clear a tab's shared zoom.
    SetPaneZoom {
        /// Expected workspace.
        workspace: WorkspaceId,
        /// The tab.
        tab: TabId,
        /// The zoomed pane; `None` clears it.
        zoomed: Option<PaneId>,
    },
    /// Reorder a tab within its workspace.
    ReorderTab {
        /// Expected workspace.
        workspace: WorkspaceId,
        /// The tab.
        tab: TabId,
        /// The tab it goes before; `None` appends.
        before: Option<TabId>,
    },
    /// Move a tab to a workspace.
    MoveTab {
        /// Expected source workspace.
        source: WorkspaceId,
        /// The tab.
        tab: TabId,
        /// The destination workspace.
        destination: WorkspaceId,
        /// The tab it goes before; `None` appends.
        before: Option<TabId>,
    },
    /// Move a session in the session order.
    MoveSession {
        /// The session.
        session: SessionId,
        /// The session it goes before; `None` appends.
        before: Option<SessionId>,
    },
    /// Move a workspace to a session.
    MoveWorkspace {
        /// Expected source session.
        source: SessionId,
        /// The workspace.
        workspace: WorkspaceId,
        /// The destination session.
        destination: SessionId,
        /// The workspace it goes before; `None` appends.
        before: Option<WorkspaceId>,
    },
    /// Move a workspace into a new session of its own.
    MoveWorkspaceToNewSession {
        /// Expected source session.
        source: SessionId,
        /// The workspace.
        workspace: WorkspaceId,
    },
    /// Exchange two panes of a tab, filed in a history slot.
    SwapPanes {
        /// The history slot.
        slot: u32,
        /// Expected workspace.
        workspace: WorkspaceId,
        /// Expected tab.
        tab: TabId,
        /// One pane.
        a: PaneId,
        /// The other pane.
        b: PaneId,
    },
    /// Move a pane, filed in a history slot.
    MovePane {
        /// The history slot.
        slot: u32,
        /// The pane.
        from: PaneRef,
        /// Where it lands.
        to: MoveDestination,
    },
    /// Drop a whole tab beside a pane of another tab, filed in a history
    /// slot.
    MoveTabToPane {
        /// The history slot.
        slot: u32,
        /// Expected workspace.
        workspace: WorkspaceId,
        /// The tab.
        tab: TabId,
        /// Where it lands.
        to: Beside,
    },
    /// Rearrange a tab's panes, filed in a history slot.
    ArrangePanes {
        /// The history slot.
        slot: u32,
        /// Expected workspace.
        workspace: WorkspaceId,
        /// The tab.
        tab: TabId,
        /// The structure the tab must still have, at most
        /// [`MAX_REQUEST_LAYOUT_ITEMS`] panes and splits.
        expected: PaneLayout,
        /// The new arrangement, at most [`MAX_REQUEST_LAYOUT_ITEMS`]
        /// nodes.
        arrangement: Arrangement,
    },
    /// Undo or redo the history entry an undo token names.
    RestoreLayout {
        /// The token.
        token: u64,
        /// Which way.
        direction: RestoreDirection,
    },
    /// Attach the connection to a session.
    AttachSession {
        /// The session.
        session: SessionId,
    },
    /// Detach one of the connection's attachments.
    DetachSession {
        /// The attachment.
        attachment: AttachmentId,
    },
    /// Point one of the connection's attachments at another session.
    RetargetAttachment {
        /// The attachment.
        attachment: AttachmentId,
        /// The session.
        session: SessionId,
    },
    /// Recheck and commit a prepared close.
    CommitClose {
        /// The ticket `PrepareClose` answered.
        ticket: u32,
        /// Whether the user confirmed the close.
        confirmed: bool,
    },
    /// Release a prepared close without committing it.
    ReleaseCloseTicket {
        /// The ticket.
        ticket: u32,
    },
    /// Run a catalog command. An identifier the server's catalog does not
    /// know answers `UnknownCommand` with its text.
    ExecuteCommand {
        /// The invocation.
        invocation: DecodedInvocation,
    },
    /// Set a viewer's terminal's history limit.
    SetScrollback {
        /// The viewer.
        viewer: ViewerId,
        /// The limit.
        limit: ScrollbackLimit,
    },
    /// Erase a viewer's terminal's history.
    ClearHistory {
        /// The viewer.
        viewer: ViewerId,
    },
    /// Reset a viewer's terminal's emulator.
    Reset {
        /// The viewer.
        viewer: ViewerId,
    },
    /// Report the grid and cell size a viewer would display.
    ReportGeometry {
        /// The viewer.
        viewer: ViewerId,
        /// The grid.
        grid: GridSize,
        /// The cell size.
        cell: CellSize,
    },
    /// Register a viewer of a terminal in one of the connection's
    /// attachments. Its result carries the viewer and the terminal's
    /// status, or, for a terminal whose runtime stopped on its own, only
    /// the final status; the viewer's first update follows on its lane.
    SubscribeTerminal {
        /// The attachment the viewer belongs to.
        attachment: AttachmentId,
        /// The terminal.
        terminal: TerminalId,
        /// What the viewer may do, and its initial state.
        options: SubscribeOptions,
    },
    /// Drop a viewer, as dropping an in-process viewer does: what it
    /// already sent still runs, and nothing more arrives on its lane.
    CloseViewer {
        /// The viewer.
        viewer: ViewerId,
    },
    /// Build a viewer's update now, credited or not, optionally after a
    /// scroll the client numbered and with a link lookup at a cell. The
    /// update arrives on the viewer's lane ahead of the result.
    Snapshot {
        /// The viewer.
        viewer: ViewerId,
        /// The scroll and the client's number for it, which typing stamps
        /// carry.
        scroll: Option<(ScrollCommand, u64)>,
        /// A cell whose link to look up against the same snapshot.
        point: Option<MousePosition>,
    },
    /// Extract text from canonical scrollback.
    Selection {
        /// The viewer.
        viewer: ViewerId,
        /// The content generation the range was computed against.
        generation: u64,
        /// The range.
        range: BufferRange,
    },
    /// Expand a multi-click selection to whole units.
    SelectUnits {
        /// The viewer.
        viewer: ViewerId,
        /// The content generation the points were computed against.
        generation: u64,
        /// Where the selection started.
        anchor: BufferPoint,
        /// Where it ends now.
        head: BufferPoint,
        /// The unit.
        unit: SelectionUnit,
    },
    /// Submit the presentation a viewer would apply.
    UpdatePresentation {
        /// The viewer.
        viewer: ViewerId,
        /// The presentation.
        presentation: Box<TerminalPresentation>,
    },
    /// Set whether a viewer's terminal compresses idle history.
    SetScrollbackCompression {
        /// The viewer.
        viewer: ViewerId,
        /// Whether to compress.
        enabled: bool,
    },
    /// Pin or unpin a tab, at the test-only minor version only.
    #[cfg(any(test, feature = "test-minor"))]
    PinTab {
        /// The tab.
        tab: TabId,
        /// Whether it is pinned.
        pinned: bool,
    },
}

impl Request {
    /// The request's kind.
    #[must_use]
    pub const fn kind(&self) -> RequestKind {
        match self {
            Self::SubscribeHierarchy => RequestKind::SubscribeHierarchy,
            Self::ResyncHierarchy => RequestKind::ResyncHierarchy,
            Self::SelectSession { .. } => RequestKind::SelectSession,
            Self::SelectWorkspace { .. } => RequestKind::SelectWorkspace,
            Self::SelectTab { .. } => RequestKind::SelectTab,
            Self::SelectPane { .. } => RequestKind::SelectPane,
            Self::PrepareClose { .. } => RequestKind::PrepareClose,
            Self::CreateSession { .. } => RequestKind::CreateSession,
            Self::CreateWorkspace { .. } => RequestKind::CreateWorkspace,
            Self::CreateWorkspaceWithTab { .. } => {
                RequestKind::CreateWorkspaceWithTab
            }
            Self::OpenTab { .. } => RequestKind::OpenTab,
            Self::SplitPane { .. } => RequestKind::SplitPane,
            Self::ClosePane { .. } => RequestKind::ClosePane,
            Self::CloseTab { .. } => RequestKind::CloseTab,
            Self::CloseWorkspace { .. } => RequestKind::CloseWorkspace,
            Self::CloseSession { .. } => RequestKind::CloseSession,
            Self::RenameSession { .. } => RequestKind::RenameSession,
            Self::RenameWorkspace { .. } => RequestKind::RenameWorkspace,
            Self::RenameTab { .. } => RequestKind::RenameTab,
            Self::RenamePane { .. } => RequestKind::RenamePane,
            Self::SetWorkspacePinned { .. } => RequestKind::SetWorkspacePinned,
            Self::ToggleInputSync { .. } => RequestKind::ToggleInputSync,
            Self::CloseEmptiedWorkspace { .. } => {
                RequestKind::CloseEmptiedWorkspace
            }
            Self::ResizeSplit { .. } => RequestKind::ResizeSplit,
            Self::SetPaneZoom { .. } => RequestKind::SetPaneZoom,
            Self::ReorderTab { .. } => RequestKind::ReorderTab,
            Self::MoveTab { .. } => RequestKind::MoveTab,
            Self::MoveSession { .. } => RequestKind::MoveSession,
            Self::MoveWorkspace { .. } => RequestKind::MoveWorkspace,
            Self::MoveWorkspaceToNewSession { .. } => {
                RequestKind::MoveWorkspaceToNewSession
            }
            Self::SwapPanes { .. } => RequestKind::SwapPanes,
            Self::MovePane { .. } => RequestKind::MovePane,
            Self::MoveTabToPane { .. } => RequestKind::MoveTabToPane,
            Self::ArrangePanes { .. } => RequestKind::ArrangePanes,
            Self::RestoreLayout { .. } => RequestKind::RestoreLayout,
            Self::AttachSession { .. } => RequestKind::AttachSession,
            Self::DetachSession { .. } => RequestKind::DetachSession,
            Self::RetargetAttachment { .. } => RequestKind::RetargetAttachment,
            Self::CommitClose { .. } => RequestKind::CommitClose,
            Self::ReleaseCloseTicket { .. } => RequestKind::ReleaseCloseTicket,
            Self::ExecuteCommand { .. } => RequestKind::ExecuteCommand,
            Self::SetScrollback { .. } => RequestKind::SetScrollback,
            Self::ClearHistory { .. } => RequestKind::ClearHistory,
            Self::Reset { .. } => RequestKind::Reset,
            Self::ReportGeometry { .. } => RequestKind::ReportGeometry,
            Self::SubscribeTerminal { .. } => RequestKind::SubscribeTerminal,
            Self::CloseViewer { .. } => RequestKind::CloseViewer,
            Self::Snapshot { .. } => RequestKind::Snapshot,
            Self::Selection { .. } => RequestKind::Selection,
            Self::SelectUnits { .. } => RequestKind::SelectUnits,
            Self::UpdatePresentation { .. } => RequestKind::UpdatePresentation,
            Self::SetScrollbackCompression { .. } => {
                RequestKind::SetScrollbackCompression
            }
            #[cfg(any(test, feature = "test-minor"))]
            Self::PinTab { .. } => RequestKind::PinTab,
        }
    }

    /// The viewer the request names, if any.
    #[must_use]
    pub const fn viewer(&self) -> Option<ViewerId> {
        match self {
            Self::SetScrollback { viewer, .. }
            | Self::ClearHistory { viewer }
            | Self::Reset { viewer }
            | Self::ReportGeometry { viewer, .. }
            | Self::CloseViewer { viewer }
            | Self::Snapshot { viewer, .. }
            | Self::Selection { viewer, .. }
            | Self::SelectUnits { viewer, .. }
            | Self::UpdatePresentation { viewer, .. }
            | Self::SetScrollbackCompression { viewer, .. } => Some(*viewer),
            _ => None,
        }
    }
}

/// A request message: the client's identity for it, and the request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestMessage {
    /// The request's identity on its connection.
    pub id: RequestId,
    /// The request.
    pub request: Request,
}

/// A client's request to abandon a request that has not started. The
/// request still gets exactly one result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Cancel {
    /// The request to abandon.
    pub request: RequestId,
}

fn direction_tag(direction: SplitDirection) -> u64 {
    match direction {
        SplitDirection::Left => 0,
        SplitDirection::Right => 1,
        SplitDirection::Up => 2,
        SplitDirection::Down => 3,
    }
}

fn read_direction(r: &mut Reader<'_>) -> Result<SplitDirection, DecodeError> {
    Ok(match r.tag()? {
        0 => SplitDirection::Left,
        1 => SplitDirection::Right,
        2 => SplitDirection::Up,
        3 => SplitDirection::Down,
        tag => {
            return Err(DecodeError::UnknownTag {
                field: "split direction",
                tag,
            });
        }
    })
}

fn write_name(
    w: &mut Writer,
    name: Option<&String>,
) -> Result<(), EncodeError> {
    w.optional_str(name.map(String::as_str), MAX_REQUEST_NAME_BYTES, "name")
}

fn read_name(r: &mut Reader<'_>) -> Result<Option<String>, DecodeError> {
    r.optional_string(MAX_REQUEST_NAME_BYTES, "name")
}

fn write_before<T: crate::codec::WireId>(
    w: &mut Writer,
    scope: IdScope,
    before: Option<T>,
) -> Result<(), EncodeError> {
    w.option(before, |w, id| w.id(scope, id))
}

fn read_before<T: crate::codec::WireId>(
    r: &mut Reader<'_>,
    scope: IdScope,
) -> Result<Option<T>, DecodeError> {
    r.option("anchor", |r| r.id(scope))
}

/// Encodes a close request: its tag, then its variant's record.
///
/// # Errors
/// Refuses an identity of another scope, or more than
/// [`MAX_CLOSE_REQUEST_TABS`] tabs.
pub fn encode_close_request(
    w: &mut Writer,
    scope: IdScope,
    request: &CloseRequest,
) -> Result<(), EncodeError> {
    match request {
        CloseRequest::Window(attachment) => {
            w.tag(0);
            w.record(|w| w.id(scope, *attachment))
        }
        CloseRequest::Session(session) => {
            w.tag(1);
            w.record(|w| w.id(scope, *session))
        }
        CloseRequest::Workspace(workspace) => {
            w.tag(2);
            w.record(|w| w.id(scope, *workspace))
        }
        CloseRequest::Tab { workspace, tab } => {
            w.tag(3);
            w.record(|w| {
                w.id(scope, *workspace)?;
                w.id(scope, *tab)
            })
        }
        CloseRequest::Pane {
            workspace,
            tab,
            pane,
        } => {
            w.tag(4);
            w.record(|w| {
                w.id(scope, *workspace)?;
                w.id(scope, *tab)?;
                w.id(scope, *pane)
            })
        }
        CloseRequest::Tabs { workspace, tabs } => {
            w.tag(5);
            w.record(|w| {
                w.id(scope, *workspace)?;
                w.seq(tabs.iter(), MAX_CLOSE_REQUEST_TABS, "tabs", |w, tab| {
                    w.id(scope, *tab)
                })
            })
        }
        CloseRequest::Application => {
            w.tag(6);
            w.record(|_| Ok(()))
        }
        CloseRequest::EmptiedWindow {
            attachment,
            workspace,
        } => {
            w.tag(7);
            w.record(|w| {
                w.id(scope, *attachment)?;
                w.id(scope, *workspace)
            })
        }
    }
}

/// Decodes a close request.
///
/// # Errors
/// Returns the first decode failure.
pub fn decode_close_request(
    r: &mut Reader<'_>,
    scope: IdScope,
) -> Result<CloseRequest, DecodeError> {
    let tag = r.tag()?;
    r.record(|r| {
        Ok(match tag {
            0 => CloseRequest::Window(r.id(scope)?),
            1 => CloseRequest::Session(r.id(scope)?),
            2 => CloseRequest::Workspace(r.id(scope)?),
            3 => CloseRequest::Tab {
                workspace: r.id(scope)?,
                tab: r.id(scope)?,
            },
            4 => CloseRequest::Pane {
                workspace: r.id(scope)?,
                tab: r.id(scope)?,
                pane: r.id(scope)?,
            },
            5 => CloseRequest::Tabs {
                workspace: r.id(scope)?,
                tabs: r.seq(MAX_CLOSE_REQUEST_TABS, "tabs", |r| r.id(scope))?,
            },
            6 => CloseRequest::Application,
            7 => CloseRequest::EmptiedWindow {
                attachment: r.id(scope)?,
                workspace: r.id(scope)?,
            },
            tag => {
                return Err(DecodeError::UnknownTag {
                    field: "close request",
                    tag,
                });
            }
        })
    })
}

/// Encodes a close effect: its tag, then its variant's record.
///
/// # Errors
/// Refuses an identity of another scope, or more than
/// [`MAX_CLOSE_REQUEST_TABS`] tabs.
pub fn encode_close_effect(
    w: &mut Writer,
    scope: IdScope,
    effect: &CloseEffect,
) -> Result<(), EncodeError> {
    match effect {
        CloseEffect::Detach(attachment) => {
            w.tag(0);
            w.record(|w| w.id(scope, *attachment))
        }
        CloseEffect::Session(session) => {
            w.tag(1);
            w.record(|w| w.id(scope, *session))
        }
        CloseEffect::Workspace(workspace) => {
            w.tag(2);
            w.record(|w| w.id(scope, *workspace))
        }
        CloseEffect::LeaveWorkspace {
            attachment,
            workspace,
        } => {
            w.tag(3);
            w.record(|w| {
                w.id(scope, *attachment)?;
                w.id(scope, *workspace)
            })
        }
        CloseEffect::Tab { workspace, tab } => {
            w.tag(4);
            w.record(|w| {
                w.id(scope, *workspace)?;
                w.id(scope, *tab)
            })
        }
        CloseEffect::Pane {
            workspace,
            tab,
            pane,
        } => {
            w.tag(5);
            w.record(|w| {
                w.id(scope, *workspace)?;
                w.id(scope, *tab)?;
                w.id(scope, *pane)
            })
        }
        CloseEffect::Tabs { workspace, tabs } => {
            w.tag(6);
            w.record(|w| {
                w.id(scope, *workspace)?;
                w.seq(tabs.iter(), MAX_CLOSE_REQUEST_TABS, "tabs", |w, tab| {
                    w.id(scope, *tab)
                })
            })
        }
        CloseEffect::Application => {
            w.tag(7);
            w.record(|_| Ok(()))
        }
    }
}

/// Decodes a close effect.
///
/// # Errors
/// Returns the first decode failure.
pub fn decode_close_effect(
    r: &mut Reader<'_>,
    scope: IdScope,
) -> Result<CloseEffect, DecodeError> {
    let tag = r.tag()?;
    r.record(|r| {
        Ok(match tag {
            0 => CloseEffect::Detach(r.id(scope)?),
            1 => CloseEffect::Session(r.id(scope)?),
            2 => CloseEffect::Workspace(r.id(scope)?),
            3 => CloseEffect::LeaveWorkspace {
                attachment: r.id(scope)?,
                workspace: r.id(scope)?,
            },
            4 => CloseEffect::Tab {
                workspace: r.id(scope)?,
                tab: r.id(scope)?,
            },
            5 => CloseEffect::Pane {
                workspace: r.id(scope)?,
                tab: r.id(scope)?,
                pane: r.id(scope)?,
            },
            6 => CloseEffect::Tabs {
                workspace: r.id(scope)?,
                tabs: r.seq(MAX_CLOSE_REQUEST_TABS, "tabs", |r| r.id(scope))?,
            },
            7 => CloseEffect::Application,
            tag => {
                return Err(DecodeError::UnknownTag {
                    field: "close effect",
                    tag,
                });
            }
        })
    })
}

fn encode_pane_ref(
    w: &mut Writer,
    scope: IdScope,
    pane: PaneRef,
) -> Result<(), EncodeError> {
    let PaneRef {
        workspace,
        tab,
        pane,
    } = pane;
    w.record(|w| {
        w.id(scope, workspace)?;
        w.id(scope, tab)?;
        w.id(scope, pane)
    })
}

fn decode_pane_ref(
    r: &mut Reader<'_>,
    scope: IdScope,
) -> Result<PaneRef, DecodeError> {
    r.record(|r| {
        Ok(PaneRef {
            workspace: r.id(scope)?,
            tab: r.id(scope)?,
            pane: r.id(scope)?,
        })
    })
}

fn encode_beside(
    w: &mut Writer,
    scope: IdScope,
    beside: Beside,
) -> Result<(), EncodeError> {
    let Beside { target, side } = beside;
    w.record(|w| {
        encode_pane_ref(w, scope, target)?;
        w.tag(direction_tag(side));
        Ok(())
    })
}

fn decode_beside(
    r: &mut Reader<'_>,
    scope: IdScope,
) -> Result<Beside, DecodeError> {
    r.record(|r| {
        Ok(Beside {
            target: decode_pane_ref(r, scope)?,
            side: read_direction(r)?,
        })
    })
}

fn encode_destination(
    w: &mut Writer,
    scope: IdScope,
    destination: MoveDestination,
) -> Result<(), EncodeError> {
    match destination {
        MoveDestination::Beside(beside) => {
            w.tag(0);
            w.record(|w| encode_beside(w, scope, beside))
        }
        MoveDestination::NewTab { workspace, before } => {
            w.tag(1);
            w.record(|w| {
                w.id(scope, workspace)?;
                write_before(w, scope, before)
            })
        }
    }
}

fn decode_destination(
    r: &mut Reader<'_>,
    scope: IdScope,
) -> Result<MoveDestination, DecodeError> {
    let tag = r.tag()?;
    r.record(|r| {
        Ok(match tag {
            0 => MoveDestination::Beside(decode_beside(r, scope)?),
            1 => MoveDestination::NewTab {
                workspace: r.id(scope)?,
                before: read_before(r, scope)?,
            },
            tag => {
                return Err(DecodeError::UnknownTag {
                    field: "move destination",
                    tag,
                });
            }
        })
    })
}

fn encode_node_id(
    w: &mut Writer,
    scope: IdScope,
    node: NodeId,
) -> Result<(), EncodeError> {
    match node {
        NodeId::Pane(pane) => {
            w.tag(0);
            w.record(|w| w.id(scope, pane))
        }
        NodeId::Split(split) => {
            w.tag(1);
            w.record(|w| w.id(scope, split))
        }
    }
}

fn decode_node_id(
    r: &mut Reader<'_>,
    scope: IdScope,
) -> Result<NodeId, DecodeError> {
    let tag = r.tag()?;
    r.record(|r| match tag {
        0 => Ok(NodeId::Pane(r.id(scope)?)),
        1 => Ok(NodeId::Split(r.id(scope)?)),
        tag => Err(DecodeError::UnknownTag {
            field: "layout node",
            tag,
        }),
    })
}

fn axis_tag(axis: SplitAxis) -> u64 {
    match axis {
        SplitAxis::Columns => 0,
        SplitAxis::Rows => 1,
    }
}

/// The nodes of an arrangement, or `None` past `MAX_LAYOUT_DEPTH`.
fn arrangement_nodes(arrangement: &Arrangement, depth: usize) -> Option<usize> {
    if depth > MAX_LAYOUT_DEPTH {
        return None;
    }
    match arrangement {
        Arrangement::Pane(_) => Some(1),
        Arrangement::Split { children, .. } => {
            children.iter().try_fold(1_usize, |nodes, (_, child)| {
                Some(nodes + arrangement_nodes(child, depth + 1)?)
            })
        }
    }
}

fn write_arrangement(
    w: &mut Writer,
    scope: IdScope,
    arrangement: &Arrangement,
) -> Result<(), EncodeError> {
    match arrangement {
        Arrangement::Pane(pane) => {
            w.tag(0);
            w.record(|w| w.id(scope, *pane))
        }
        Arrangement::Split { axis, children } => {
            w.tag(1);
            w.record(|w| {
                w.tag(axis_tag(*axis));
                w.seq(
                    children.iter(),
                    MAX_SPLIT_CHILDREN,
                    "arrangement children",
                    |w, (weight, child)| {
                        w.record(|w| {
                            w.u32(*weight);
                            write_arrangement(w, scope, child)
                        })
                    },
                )
            })
        }
    }
}

/// Encodes an arrangement of at most [`MAX_REQUEST_LAYOUT_ITEMS`] nodes and
/// at most `MAX_LAYOUT_DEPTH` deep.
///
/// # Errors
/// Refuses a larger or deeper arrangement, a split of more than
/// `MAX_SPLIT_CHILDREN` children, or an identity of another scope.
pub fn encode_arrangement(
    w: &mut Writer,
    scope: IdScope,
    arrangement: &Arrangement,
) -> Result<(), EncodeError> {
    let nodes = arrangement_nodes(arrangement, 1)
        .ok_or(EncodeError::Invalid("arrangement depth"))?;
    if nodes > MAX_REQUEST_LAYOUT_ITEMS {
        return Err(EncodeError::LimitExceeded {
            field: "arrangement",
        });
    }
    write_arrangement(w, scope, arrangement)
}

fn read_arrangement(
    r: &mut Reader<'_>,
    scope: IdScope,
    depth: usize,
    items: &mut Items,
) -> Result<Arrangement, DecodeError> {
    if depth > MAX_LAYOUT_DEPTH {
        return Err(DecodeError::Invalid("arrangement depth"));
    }
    items.add(1)?;
    let tag = r.tag()?;
    r.record(|r| match tag {
        0 => Ok(Arrangement::Pane(r.id(scope)?)),
        1 => {
            let axis = match r.tag()? {
                0 => SplitAxis::Columns,
                1 => SplitAxis::Rows,
                tag => {
                    return Err(DecodeError::UnknownTag {
                        field: "split axis",
                        tag,
                    });
                }
            };
            let children =
                r.seq(MAX_SPLIT_CHILDREN, "arrangement children", |r| {
                    r.record(|r| {
                        let weight = r.u32()?;
                        Ok((
                            weight,
                            read_arrangement(r, scope, depth + 1, items)?,
                        ))
                    })
                })?;
            Ok(Arrangement::Split { axis, children })
        }
        tag => Err(DecodeError::UnknownTag {
            field: "arrangement",
            tag,
        }),
    })
}

/// Decodes an arrangement, refusing more than [`MAX_REQUEST_LAYOUT_ITEMS`]
/// nodes and checking the depth before it recurses.
///
/// # Errors
/// Returns the first decode failure.
pub fn decode_arrangement(
    r: &mut Reader<'_>,
    scope: IdScope,
) -> Result<Arrangement, DecodeError> {
    let mut items =
        Items::limited(MAX_REQUEST_LAYOUT_ITEMS, "arrangement node count");
    read_arrangement(r, scope, 1, &mut items)
}

/// Encodes a layout a request carries, refusing one of more than
/// [`MAX_REQUEST_LAYOUT_ITEMS`] panes and splits.
fn encode_request_layout(
    w: &mut Writer,
    scope: IdScope,
    layout: &PaneLayout,
) -> Result<(), EncodeError> {
    if layout.pane_count() + layout.split_ids().len() > MAX_REQUEST_LAYOUT_ITEMS
    {
        return Err(EncodeError::LimitExceeded { field: "layout" });
    }
    encode_layout(w, scope, layout)
}

fn decode_request_layout(
    r: &mut Reader<'_>,
    scope: IdScope,
) -> Result<PaneLayout, DecodeError> {
    let mut items =
        Items::limited(MAX_REQUEST_LAYOUT_ITEMS, "layout item count");
    decode_layout_counted(r, scope, &mut items)
}

fn restore_tag(direction: RestoreDirection) -> u64 {
    match direction {
        RestoreDirection::Undo => 0,
        RestoreDirection::Redo => 1,
    }
}

/// Encodes a request's fields, without its tag.
#[expect(clippy::too_many_lines, reason = "one short arm per request kind")]
fn encode_fields(
    w: &mut Writer,
    scope: IdScope,
    request: &Request,
) -> Result<(), EncodeError> {
    match request {
        Request::SubscribeHierarchy | Request::ResyncHierarchy => Ok(()),
        Request::SelectSession { session }
        | Request::CloseSession { session }
        | Request::AttachSession { session } => w.id(scope, *session),
        Request::SelectWorkspace { workspace }
        | Request::CloseWorkspace { workspace } => w.id(scope, *workspace),
        Request::SelectTab { tab } => w.id(scope, *tab),
        Request::SelectPane { pane } => w.id(scope, *pane),
        Request::PrepareClose { request } => {
            encode_close_request(w, scope, request)
        }
        Request::CreateSession { name } => write_name(w, name.as_ref()),
        Request::CreateWorkspace { session, name } => {
            w.id(scope, *session)?;
            write_name(w, name.as_ref())
        }
        Request::CreateWorkspaceWithTab {
            session,
            name,
            command,
        } => {
            w.id(scope, *session)?;
            write_name(w, name.as_ref())?;
            encode_command(w, command)
        }
        Request::OpenTab { workspace, command } => {
            w.id(scope, *workspace)?;
            encode_command(w, command)
        }
        Request::SplitPane {
            workspace,
            tab,
            pane,
            direction,
            command,
        } => {
            w.id(scope, *workspace)?;
            w.id(scope, *tab)?;
            w.id(scope, *pane)?;
            w.tag(direction_tag(*direction));
            encode_command(w, command)
        }
        Request::ClosePane {
            workspace,
            tab,
            pane,
        } => {
            w.id(scope, *workspace)?;
            w.id(scope, *tab)?;
            w.id(scope, *pane)
        }
        Request::CloseTab { workspace, tab }
        | Request::ToggleInputSync { workspace, tab } => {
            w.id(scope, *workspace)?;
            w.id(scope, *tab)
        }
        Request::RenameSession { session, name } => {
            w.id(scope, *session)?;
            write_name(w, name.as_ref())
        }
        Request::RenameWorkspace { workspace, name } => {
            w.id(scope, *workspace)?;
            write_name(w, name.as_ref())
        }
        Request::RenameTab { tab, name } => {
            w.id(scope, *tab)?;
            write_name(w, name.as_ref())
        }
        Request::RenamePane { pane, name } => {
            w.id(scope, *pane)?;
            write_name(w, name.as_ref())
        }
        Request::SetWorkspacePinned { workspace, pinned } => {
            w.id(scope, *workspace)?;
            w.bool(*pinned);
            Ok(())
        }
        Request::CloseEmptiedWorkspace { session, workspace }
        | Request::MoveWorkspaceToNewSession {
            source: session,
            workspace,
        } => {
            w.id(scope, *session)?;
            w.id(scope, *workspace)
        }
        Request::ResizeSplit {
            workspace,
            tab,
            split,
            children,
            weights,
        } => {
            w.id(scope, *workspace)?;
            w.id(scope, *tab)?;
            w.id(scope, *split)?;
            w.seq(
                children.iter(),
                MAX_SPLIT_CHILDREN,
                "split children",
                |w, node| encode_node_id(w, scope, *node),
            )?;
            w.seq(
                weights.iter(),
                MAX_SPLIT_CHILDREN,
                "split weights",
                |w, weight| {
                    w.u32(*weight);
                    Ok(())
                },
            )
        }
        Request::SetPaneZoom {
            workspace,
            tab,
            zoomed,
        } => {
            w.id(scope, *workspace)?;
            w.id(scope, *tab)?;
            w.option(*zoomed, |w, pane| w.id(scope, pane))
        }
        Request::ReorderTab {
            workspace,
            tab,
            before,
        } => {
            w.id(scope, *workspace)?;
            w.id(scope, *tab)?;
            write_before(w, scope, *before)
        }
        Request::MoveTab {
            source,
            tab,
            destination,
            before,
        } => {
            w.id(scope, *source)?;
            w.id(scope, *tab)?;
            w.id(scope, *destination)?;
            write_before(w, scope, *before)
        }
        Request::MoveSession { session, before } => {
            w.id(scope, *session)?;
            write_before(w, scope, *before)
        }
        Request::MoveWorkspace {
            source,
            workspace,
            destination,
            before,
        } => {
            w.id(scope, *source)?;
            w.id(scope, *workspace)?;
            w.id(scope, *destination)?;
            write_before(w, scope, *before)
        }
        Request::SwapPanes {
            slot,
            workspace,
            tab,
            a,
            b,
        } => {
            w.u32(*slot);
            w.id(scope, *workspace)?;
            w.id(scope, *tab)?;
            w.id(scope, *a)?;
            w.id(scope, *b)
        }
        Request::MovePane { slot, from, to } => {
            w.u32(*slot);
            encode_pane_ref(w, scope, *from)?;
            encode_destination(w, scope, *to)
        }
        Request::MoveTabToPane {
            slot,
            workspace,
            tab,
            to,
        } => {
            w.u32(*slot);
            w.id(scope, *workspace)?;
            w.id(scope, *tab)?;
            encode_beside(w, scope, *to)
        }
        Request::ArrangePanes {
            slot,
            workspace,
            tab,
            expected,
            arrangement,
        } => {
            w.u32(*slot);
            w.id(scope, *workspace)?;
            w.id(scope, *tab)?;
            encode_request_layout(w, scope, expected)?;
            encode_arrangement(w, scope, arrangement)
        }
        Request::RestoreLayout { token, direction } => {
            w.u64(*token);
            w.tag(restore_tag(*direction));
            Ok(())
        }
        Request::DetachSession { attachment } => w.id(scope, *attachment),
        Request::RetargetAttachment {
            attachment,
            session,
        } => {
            w.id(scope, *attachment)?;
            w.id(scope, *session)
        }
        Request::CommitClose { ticket, confirmed } => {
            w.u32(*ticket);
            w.bool(*confirmed);
            Ok(())
        }
        Request::ReleaseCloseTicket { ticket } => {
            w.u32(*ticket);
            Ok(())
        }
        Request::ExecuteCommand { invocation } => {
            encode_decoded_invocation(w, scope, invocation)
        }
        Request::SetScrollback { viewer, limit } => {
            w.id(scope, *viewer)?;
            encode_scrollback(w, *limit)
        }
        Request::ClearHistory { viewer } | Request::Reset { viewer } => {
            w.id(scope, *viewer)
        }
        Request::ReportGeometry { viewer, grid, cell } => {
            w.id(scope, *viewer)?;
            encode_grid(w, *grid);
            encode_cell_size(w, *cell);
            Ok(())
        }
        Request::SubscribeTerminal {
            attachment,
            terminal,
            options,
        } => {
            w.id(scope, *attachment)?;
            w.id(scope, *terminal)?;
            encode_subscribe_options(w, options)
        }
        Request::CloseViewer { viewer } => w.id(scope, *viewer),
        Request::Snapshot {
            viewer,
            scroll,
            point,
        } => {
            w.id(scope, *viewer)?;
            w.option(*scroll, |w, (scroll, number)| {
                write_scroll(w, scroll);
                w.u64(number);
                Ok(())
            })?;
            w.option(*point, |w, point| {
                write_position(w, point);
                Ok(())
            })
        }
        Request::Selection {
            viewer,
            generation,
            range,
        } => {
            w.id(scope, *viewer)?;
            w.u64(*generation);
            write_range(w, *range)
        }
        Request::SelectUnits {
            viewer,
            generation,
            anchor,
            head,
            unit,
        } => {
            w.id(scope, *viewer)?;
            w.u64(*generation);
            write_point(w, *anchor)?;
            write_point(w, *head)?;
            write_unit(w, *unit);
            Ok(())
        }
        Request::UpdatePresentation {
            viewer,
            presentation,
        } => {
            w.id(scope, *viewer)?;
            encode_presentation(w, presentation)
        }
        Request::SetScrollbackCompression { viewer, enabled } => {
            w.id(scope, *viewer)?;
            w.bool(*enabled);
            Ok(())
        }
        #[cfg(any(test, feature = "test-minor"))]
        Request::PinTab { tab, pinned } => {
            w.id(scope, *tab)?;
            w.bool(*pinned);
            Ok(())
        }
    }
}

/// Decodes the fields of a request of `kind`.
#[expect(clippy::too_many_lines, reason = "one short arm per request kind")]
fn decode_fields(
    r: &mut Reader<'_>,
    scope: IdScope,
    kind: RequestKind,
) -> Result<Request, DecodeError> {
    Ok(match kind {
        RequestKind::SubscribeHierarchy => Request::SubscribeHierarchy,
        RequestKind::ResyncHierarchy => Request::ResyncHierarchy,
        RequestKind::SelectSession => Request::SelectSession {
            session: r.id(scope)?,
        },
        RequestKind::SelectWorkspace => Request::SelectWorkspace {
            workspace: r.id(scope)?,
        },
        RequestKind::SelectTab => Request::SelectTab { tab: r.id(scope)? },
        RequestKind::SelectPane => Request::SelectPane { pane: r.id(scope)? },
        RequestKind::PrepareClose => Request::PrepareClose {
            request: decode_close_request(r, scope)?,
        },
        RequestKind::CreateSession => Request::CreateSession {
            name: read_name(r)?,
        },
        RequestKind::CreateWorkspace => Request::CreateWorkspace {
            session: r.id(scope)?,
            name: read_name(r)?,
        },
        RequestKind::CreateWorkspaceWithTab => {
            Request::CreateWorkspaceWithTab {
                session: r.id(scope)?,
                name: read_name(r)?,
                command: Box::new(decode_command(r)?),
            }
        }
        RequestKind::OpenTab => Request::OpenTab {
            workspace: r.id(scope)?,
            command: Box::new(decode_command(r)?),
        },
        RequestKind::SplitPane => Request::SplitPane {
            workspace: r.id(scope)?,
            tab: r.id(scope)?,
            pane: r.id(scope)?,
            direction: read_direction(r)?,
            command: Box::new(decode_command(r)?),
        },
        RequestKind::ClosePane => Request::ClosePane {
            workspace: r.id(scope)?,
            tab: r.id(scope)?,
            pane: r.id(scope)?,
        },
        RequestKind::CloseTab => Request::CloseTab {
            workspace: r.id(scope)?,
            tab: r.id(scope)?,
        },
        RequestKind::CloseWorkspace => Request::CloseWorkspace {
            workspace: r.id(scope)?,
        },
        RequestKind::CloseSession => Request::CloseSession {
            session: r.id(scope)?,
        },
        RequestKind::RenameSession => Request::RenameSession {
            session: r.id(scope)?,
            name: read_name(r)?,
        },
        RequestKind::RenameWorkspace => Request::RenameWorkspace {
            workspace: r.id(scope)?,
            name: read_name(r)?,
        },
        RequestKind::RenameTab => Request::RenameTab {
            tab: r.id(scope)?,
            name: read_name(r)?,
        },
        RequestKind::RenamePane => Request::RenamePane {
            pane: r.id(scope)?,
            name: read_name(r)?,
        },
        RequestKind::SetWorkspacePinned => Request::SetWorkspacePinned {
            workspace: r.id(scope)?,
            pinned: r.bool()?,
        },
        RequestKind::ToggleInputSync => Request::ToggleInputSync {
            workspace: r.id(scope)?,
            tab: r.id(scope)?,
        },
        RequestKind::CloseEmptiedWorkspace => Request::CloseEmptiedWorkspace {
            session: r.id(scope)?,
            workspace: r.id(scope)?,
        },
        RequestKind::ResizeSplit => Request::ResizeSplit {
            workspace: r.id(scope)?,
            tab: r.id(scope)?,
            split: r.id(scope)?,
            children: r.seq(MAX_SPLIT_CHILDREN, "split children", |r| {
                decode_node_id(r, scope)
            })?,
            weights: r.seq(MAX_SPLIT_CHILDREN, "split weights", Reader::u32)?,
        },
        RequestKind::SetPaneZoom => Request::SetPaneZoom {
            workspace: r.id(scope)?,
            tab: r.id(scope)?,
            zoomed: r.option("zoomed pane", |r| r.id(scope))?,
        },
        RequestKind::ReorderTab => Request::ReorderTab {
            workspace: r.id(scope)?,
            tab: r.id(scope)?,
            before: read_before(r, scope)?,
        },
        RequestKind::MoveTab => Request::MoveTab {
            source: r.id(scope)?,
            tab: r.id(scope)?,
            destination: r.id(scope)?,
            before: read_before(r, scope)?,
        },
        RequestKind::MoveSession => Request::MoveSession {
            session: r.id(scope)?,
            before: read_before(r, scope)?,
        },
        RequestKind::MoveWorkspace => Request::MoveWorkspace {
            source: r.id(scope)?,
            workspace: r.id(scope)?,
            destination: r.id(scope)?,
            before: read_before(r, scope)?,
        },
        RequestKind::MoveWorkspaceToNewSession => {
            Request::MoveWorkspaceToNewSession {
                source: r.id(scope)?,
                workspace: r.id(scope)?,
            }
        }
        RequestKind::SwapPanes => Request::SwapPanes {
            slot: r.u32()?,
            workspace: r.id(scope)?,
            tab: r.id(scope)?,
            a: r.id(scope)?,
            b: r.id(scope)?,
        },
        RequestKind::MovePane => Request::MovePane {
            slot: r.u32()?,
            from: decode_pane_ref(r, scope)?,
            to: decode_destination(r, scope)?,
        },
        RequestKind::MoveTabToPane => Request::MoveTabToPane {
            slot: r.u32()?,
            workspace: r.id(scope)?,
            tab: r.id(scope)?,
            to: decode_beside(r, scope)?,
        },
        RequestKind::ArrangePanes => Request::ArrangePanes {
            slot: r.u32()?,
            workspace: r.id(scope)?,
            tab: r.id(scope)?,
            expected: decode_request_layout(r, scope)?,
            arrangement: decode_arrangement(r, scope)?,
        },
        RequestKind::RestoreLayout => Request::RestoreLayout {
            token: r.u64()?,
            direction: match r.tag()? {
                0 => RestoreDirection::Undo,
                1 => RestoreDirection::Redo,
                tag => {
                    return Err(DecodeError::UnknownTag {
                        field: "restore direction",
                        tag,
                    });
                }
            },
        },
        RequestKind::AttachSession => Request::AttachSession {
            session: r.id(scope)?,
        },
        RequestKind::DetachSession => Request::DetachSession {
            attachment: r.id(scope)?,
        },
        RequestKind::RetargetAttachment => Request::RetargetAttachment {
            attachment: r.id(scope)?,
            session: r.id(scope)?,
        },
        RequestKind::CommitClose => Request::CommitClose {
            ticket: r.u32()?,
            confirmed: r.bool()?,
        },
        RequestKind::ReleaseCloseTicket => {
            Request::ReleaseCloseTicket { ticket: r.u32()? }
        }
        RequestKind::ExecuteCommand => Request::ExecuteCommand {
            invocation: decode_invocation(r, scope)?,
        },
        RequestKind::SetScrollback => Request::SetScrollback {
            viewer: r.id(scope)?,
            limit: decode_scrollback(r)?,
        },
        RequestKind::ClearHistory => Request::ClearHistory {
            viewer: r.id(scope)?,
        },
        RequestKind::Reset => Request::Reset {
            viewer: r.id(scope)?,
        },
        RequestKind::ReportGeometry => Request::ReportGeometry {
            viewer: r.id(scope)?,
            grid: decode_grid(r)?,
            cell: decode_cell_size(r)?,
        },
        RequestKind::SubscribeTerminal => Request::SubscribeTerminal {
            attachment: r.id(scope)?,
            terminal: r.id(scope)?,
            options: decode_subscribe_options(r)?,
        },
        RequestKind::CloseViewer => Request::CloseViewer {
            viewer: r.id(scope)?,
        },
        RequestKind::Snapshot => Request::Snapshot {
            viewer: r.id(scope)?,
            scroll: r.option("scroll", |r| Ok((read_scroll(r)?, r.u64()?)))?,
            point: r.option("point", read_position)?,
        },
        RequestKind::Selection => Request::Selection {
            viewer: r.id(scope)?,
            generation: r.u64()?,
            range: read_range(r)?,
        },
        RequestKind::SelectUnits => Request::SelectUnits {
            viewer: r.id(scope)?,
            generation: r.u64()?,
            anchor: read_point(r)?,
            head: read_point(r)?,
            unit: read_unit(r)?,
        },
        RequestKind::UpdatePresentation => Request::UpdatePresentation {
            viewer: r.id(scope)?,
            presentation: Box::new(decode_presentation(r)?),
        },
        RequestKind::SetScrollbackCompression => {
            Request::SetScrollbackCompression {
                viewer: r.id(scope)?,
                enabled: r.bool()?,
            }
        }
        #[cfg(any(test, feature = "test-minor"))]
        RequestKind::PinTab => Request::PinTab {
            tab: r.id(scope)?,
            pinned: r.bool()?,
        },
    })
}

/// Encodes a request message record: its identity, then the request's tag
/// and record.
///
/// # Errors
/// Refuses a request newer than `minor` with [`EncodeError::Unsupported`],
/// an identity of another scope, and a field past its limit.
pub fn encode_request(
    w: &mut Writer,
    scope: IdScope,
    message: &RequestMessage,
    minor: u16,
) -> Result<(), EncodeError> {
    let RequestMessage { id, request } = message;
    let kind = request.kind();
    if kind.since() > minor {
        return Err(EncodeError::Unsupported);
    }
    w.record(|w| {
        write_request_id(w, *id);
        w.tag(kind.number());
        w.record(|w| encode_fields(w, scope, request))
    })
}

/// Decodes a request message record at negotiated minor version `minor`.
///
/// # Errors
/// Returns [`DecodeError::Invalid`] for a zero request identity,
/// [`DecodeError::UnknownTag`] for a request the version does not define,
/// or the first field failure.
pub fn decode_request(
    r: &mut Reader<'_>,
    scope: IdScope,
    minor: u16,
) -> Result<RequestMessage, DecodeError> {
    r.record(|r| {
        let id = read_request_id(r)?;
        let tag = r.tag()?;
        let kind = RequestKind::from_number(tag)
            .filter(|kind| kind.since() <= minor)
            .ok_or(DecodeError::UnknownTag {
                field: "request",
                tag,
            })?;
        let request = r.record(|r| decode_fields(r, scope, kind))?;
        Ok(RequestMessage { id, request })
    })
}

/// Encodes a cancel record.
///
/// # Errors
/// Never fails today; the signature matches the other encoders.
pub fn encode_cancel(
    w: &mut Writer,
    cancel: Cancel,
) -> Result<(), EncodeError> {
    let Cancel { request } = cancel;
    w.record(|w| {
        write_request_id(w, request);
        Ok(())
    })
}

/// Decodes a cancel record.
///
/// # Errors
/// Returns [`DecodeError::Invalid`] for a zero request identity, or a
/// varint error.
pub fn decode_cancel(r: &mut Reader<'_>) -> Result<Cancel, DecodeError> {
    r.record(|r| {
        Ok(Cancel {
            request: read_request_id(r)?,
        })
    })
}

#[cfg(test)]
mod tests;
