//! Golden vectors: canonical values and their bytes for each minor version.
//!
//! A released minor version's bytes never change, so a test fails when an
//! existing vector's bytes do. A new minor version adds vectors instead of
//! editing these. The module builds for `wasm32-unknown-unknown` behind the
//! `vectors` feature, and the native tests decode every vector.

use std::collections::BTreeSet;

use huterm_protocol::{
    AttachmentId, ClipboardDestination, CommandArgument, CommandInvocation,
    CommandOutcome, CommandValue, HierarchyEvent, HierarchyState, PaneId,
    PaneInfo, PaneLayout, PaneNode, RuntimeId, SPLIT_TOTAL, ServerIncarnation,
    SessionId, SessionInfo, SplitAxis, SplitChild, SplitId, SplitNode,
    StreamId, TabChange, TabId, TabInfo, TerminalId, ViewerId, WorkspaceId,
    WorkspaceInfo, ids,
};

use crate::caps::{Direction, MessageCaps};
use crate::codec::IdScope;
use crate::error::{ErrorCode, Retry, WireError};
use crate::frame::{Frame, FrameEnd};
use crate::handshake::{
    ClientKind, Feature, Hello, Reject, RejectReason, ServerId,
    TransportProfile, Welcome,
};
use crate::hierarchy::{HierarchyContext, HierarchySnapshot, SequencedEvent};
use crate::message::{
    DecodeContext, EffectPayload, HostEffect, Message, Paste,
};

/// The client-local runtime the vectors' identities are scoped to.
pub const RUNTIME: RuntimeId = RuntimeId::new(1);

/// The server incarnation of the vectors' connection.
pub const INCARNATION: ServerIncarnation = ServerIncarnation::from_bytes([
    0x10, 0x32, 0x54, 0x76, 0x98, 0xba, 0xdc, 0xfe, 0x01, 0x23, 0x45, 0x67,
    0x89, 0xab, 0xcd, 0xef,
]);

/// The vectors' identity scope.
#[must_use]
pub const fn scope() -> IdScope {
    IdScope::new(RUNTIME)
}

/// The decode context of the vectors' connection at `minor`.
#[must_use]
pub const fn context(minor: u16) -> DecodeContext {
    DecodeContext {
        hierarchy: HierarchyContext {
            stream: StreamId::remote(RUNTIME, INCARNATION),
        },
        caps: MessageCaps::for_minor(minor),
    }
}

/// One message and its canonical bytes.
#[derive(Clone, Debug)]
pub struct MessageVector {
    /// What the vector shows.
    pub name: &'static str,
    /// Which side sends the message.
    pub from: Direction,
    /// The value.
    pub message: Message,
    /// Its bytes.
    pub bytes: &'static [u8],
}

/// A record that travels inside a later slice's messages.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Record {
    /// A catalog invocation.
    Invocation(CommandInvocation),
    /// A command's outcome.
    Outcome(CommandOutcome),
    /// A request's failure.
    Error(WireError),
}

/// One record and its canonical bytes.
#[derive(Clone, Debug)]
pub struct RecordVector {
    /// What the vector shows.
    pub name: &'static str,
    /// The value.
    pub record: Record,
    /// Its bytes.
    pub bytes: &'static [u8],
}

/// One frame and its canonical bytes.
#[derive(Clone, Debug)]
pub struct FrameVector {
    /// What the vector shows.
    pub name: &'static str,
    /// The value.
    pub frame: Frame,
    /// Its bytes, length prefix included.
    pub bytes: &'static [u8],
}

fn session_id(value: u64) -> SessionId {
    SessionId::in_runtime(RUNTIME, value)
}

fn workspace_id(value: u64) -> WorkspaceId {
    WorkspaceId::in_runtime(RUNTIME, value)
}

fn tab_id(value: u64) -> TabId {
    TabId::in_runtime(RUNTIME, value)
}

fn pane_id(value: u64) -> PaneId {
    PaneId::in_runtime(RUNTIME, value)
}

fn pane(value: u64, custom_name: Option<&str>) -> PaneInfo {
    PaneInfo {
        id: pane_id(value),
        terminal_id: TerminalId::in_runtime(RUNTIME, value + 1),
        fallback_name: "zsh".into(),
        custom_name: custom_name.map(str::to_owned),
    }
}

/// Panes 5 and 7 side by side under split 9, the first named "logs".
///
/// # Panics
/// Never: the layout is valid by construction.
#[must_use]
pub fn split_layout() -> PaneLayout {
    PaneLayout::new(PaneNode::Split(SplitNode {
        id: SplitId::in_runtime(RUNTIME, 9),
        axis: SplitAxis::Columns,
        children: vec![
            SplitChild {
                weight: SPLIT_TOTAL / 2,
                node: PaneNode::Pane(pane(5, Some("logs"))),
            },
            SplitChild {
                weight: SPLIT_TOTAL / 2,
                node: PaneNode::Pane(pane(7, None)),
            },
        ],
    }))
    .expect("the vector layout is valid")
}

fn split_tab() -> TabInfo {
    TabInfo {
        id: tab_id(4),
        custom_name: Some("build".into()),
        layout: split_layout(),
        zoomed: Some(pane_id(7)),
        input_sync: true,
    }
}

fn single_tab() -> TabInfo {
    TabInfo {
        id: tab_id(10),
        custom_name: None,
        layout: PaneLayout::single(pane(11, None)),
        zoomed: None,
        input_sync: false,
    }
}

/// A hierarchy of two sessions, the first with a workspace of two tabs,
/// one split, and two attachments, current through sequence 42.
///
/// # Panics
/// Never: the hierarchy is valid by construction.
#[must_use]
pub fn sample_hierarchy() -> HierarchyState {
    HierarchyState::from_sessions(
        context(0).hierarchy.stream,
        42,
        [
            (
                SessionInfo {
                    id: session_id(1),
                    custom_name: Some("work".into()),
                    automatic_name: "Session 1".into(),
                },
                vec![(
                    WorkspaceInfo {
                        id: workspace_id(2),
                        custom_name: None,
                        automatic_name: "Workspace 1".into(),
                    },
                    vec![split_tab(), single_tab()],
                )],
            ),
            (
                SessionInfo {
                    id: session_id(3),
                    custom_name: None,
                    automatic_name: "Session 2".into(),
                },
                Vec::new(),
            ),
        ],
        [
            (AttachmentId::in_runtime(RUNTIME, 13), session_id(1)),
            (AttachmentId::in_runtime(RUNTIME, 14), session_id(3)),
        ],
    )
    .expect("the vector hierarchy is valid")
}

/// The digest of [`sample_hierarchy`] under the base profile, which pins
/// `HierarchyState::digest`'s documented encoding.
pub const SAMPLE_HIERARCHY_DIGEST: u64 = 0x528b_c73a_f88e_e537;

/// One event of every variant, in tag order, each valid on its own.
#[must_use]
#[expect(clippy::too_many_lines, reason = "one short value per event kind")]
pub fn every_event() -> Vec<HierarchyEvent> {
    vec![
        HierarchyEvent::SessionCreated {
            session: SessionInfo {
                id: session_id(20),
                custom_name: None,
                automatic_name: "Session 3".into(),
            },
            index: 2,
        },
        HierarchyEvent::SessionRenamed {
            session: session_id(1),
            custom_name: Some("play".into()),
        },
        HierarchyEvent::SessionClosed {
            session: session_id(3),
        },
        HierarchyEvent::WorkspaceCreated {
            session: session_id(1),
            workspace: WorkspaceInfo {
                id: workspace_id(21),
                custom_name: Some("docs".into()),
                automatic_name: "Workspace 2".into(),
            },
            index: 1,
        },
        HierarchyEvent::WorkspaceRenamed {
            workspace: workspace_id(2),
            custom_name: None,
        },
        HierarchyEvent::WorkspaceMoved {
            workspace: workspace_id(2),
            session: session_id(3),
            index: 0,
        },
        HierarchyEvent::WorkspaceClosed {
            workspace: workspace_id(2),
        },
        HierarchyEvent::TabOpened {
            workspace: workspace_id(2),
            tab: TabInfo {
                id: tab_id(22),
                layout: PaneLayout::single(pane(23, None)),
                ..single_tab()
            },
            index: 2,
        },
        HierarchyEvent::TabRenamed {
            tab: tab_id(10),
            custom_name: Some("tests".into()),
        },
        HierarchyEvent::PaneRenamed {
            pane: pane_id(7),
            custom_name: Some("shell".into()),
        },
        HierarchyEvent::TabMoved {
            tab: tab_id(10),
            workspace: workspace_id(2),
            index: 0,
        },
        HierarchyEvent::TabClosed { tab: tab_id(10) },
        HierarchyEvent::PaneLayoutChanged {
            tab: tab_id(4),
            layout: PaneLayout::single(pane(5, Some("logs"))),
            zoomed: None,
        },
        HierarchyEvent::TabInputSyncChanged {
            tab: tab_id(4),
            input_sync: false,
        },
        HierarchyEvent::PaneZoomChanged {
            tab: tab_id(4),
            zoomed: None,
        },
        HierarchyEvent::PanesRearranged {
            changes: vec![
                TabChange::Layout {
                    tab: tab_id(4),
                    layout: PaneLayout::single(pane(5, Some("logs"))),
                    zoomed: None,
                },
                TabChange::Opened {
                    workspace: workspace_id(2),
                    tab: TabInfo {
                        id: tab_id(24),
                        layout: PaneLayout::single(pane(7, None)),
                        ..single_tab()
                    },
                    index: 1,
                },
                TabChange::Placed {
                    tab: tab_id(10),
                    workspace: workspace_id(2),
                    index: 2,
                },
                TabChange::Removed { tab: tab_id(25) },
            ],
        },
        HierarchyEvent::AttachmentCreated {
            attachment: AttachmentId::in_runtime(RUNTIME, 26),
            session: session_id(1),
        },
        HierarchyEvent::AttachmentRetargeted {
            attachment: AttachmentId::in_runtime(RUNTIME, 13),
            session: session_id(3),
        },
        HierarchyEvent::AttachmentDetached {
            attachment: AttachmentId::in_runtime(RUNTIME, 14),
        },
        HierarchyEvent::Reset,
        HierarchyEvent::Skipped,
    ]
}

/// The minor version 0 `Hello` the vectors send.
#[must_use]
pub fn hello() -> Hello {
    Hello {
        major: crate::MAJOR,
        minor: 0,
        features: BTreeSet::from([Feature::RICH_CONTENT]),
        client_kind: ClientKind::Desktop,
        max_frame: crate::frame::DEFAULT_MAX_FRAME,
        name: "huterm".into(),
        build_version: "0.31.0".into(),
    }
}

/// The minor version 0 `Welcome` that answers [`hello`].
#[must_use]
pub fn welcome() -> Welcome {
    Welcome {
        major: crate::MAJOR,
        minor: 0,
        features: BTreeSet::new(),
        incarnation: INCARNATION,
        server_id: ServerId([0xa5; 16]),
        host_name: "devbox".into(),
        socket_name: "default".into(),
        build_version: "0.31.0".into(),
        process_id: 4242,
        max_frame: crate::frame::DEFAULT_MAX_FRAME,
        profile: TransportProfile::local(0),
    }
}

/// Every minor version 0 message vector, with its bytes.
#[must_use]
pub fn minor_0_messages() -> Vec<MessageVector> {
    let from_server = |name, message, bytes| MessageVector {
        name,
        from: Direction::ServerToClient,
        message,
        bytes,
    };
    let from_client = |name, message, bytes| MessageVector {
        name,
        from: Direction::ClientToServer,
        message,
        bytes,
    };
    let state = sample_hierarchy();
    let mut vectors = vec![
        from_client("hello", Message::Hello(hello()), HELLO),
        from_server("welcome", Message::Welcome(welcome()), WELCOME),
        from_server(
            "reject incompatible major",
            Message::Reject(Reject {
                reason: RejectReason::IncompatibleMajor { major: 1, minor: 0 },
                message: "upgrade".into(),
            }),
            REJECT_MAJOR,
        ),
        from_server(
            "reject busy",
            Message::Reject(Reject {
                reason: RejectReason::ServerBusy,
                message: String::new(),
            }),
            REJECT_BUSY,
        ),
        from_server(
            "reject malformed",
            Message::Reject(Reject {
                reason: RejectReason::Malformed,
                message: "bad".into(),
            }),
            REJECT_MALFORMED,
        ),
        from_server(
            "hierarchy snapshot",
            Message::HierarchySnapshot(HierarchySnapshot {
                digest: state.digest(),
                state: state.clone(),
            }),
            HIERARCHY_SNAPSHOT,
        ),
        from_server(
            "hierarchy resync",
            Message::HierarchyResync(HierarchySnapshot {
                digest: state.digest(),
                state,
            }),
            HIERARCHY_RESYNC,
        ),
        from_client(
            "paste",
            Message::Paste(Paste {
                viewer: ViewerId::in_runtime(RUNTIME, 30),
                seq: 7,
                text: "echo hi\n".into(),
            }),
            PASTE,
        ),
        from_server(
            "clipboard write",
            Message::HostEffect(HostEffect {
                effect: 1,
                viewer: ViewerId::in_runtime(RUNTIME, 30),
                permission_generation: 2,
                terminal: TerminalId::in_runtime(RUNTIME, 6),
                payload: EffectPayload::ClipboardWrite {
                    destination: ClipboardDestination::System,
                    text: "copied".into(),
                },
                response_deadline_ms: None,
            }),
            CLIPBOARD_WRITE,
        ),
    ];
    for ((event, bytes), seq) in every_event().into_iter().zip(EVENTS).zip(43..)
    {
        vectors.push(from_server(
            "hierarchy event",
            Message::HierarchyEvent(SequencedEvent {
                seq,
                digest: (seq == 43).then_some(0x0123_4567_89ab_cdef),
                event,
            }),
            bytes,
        ));
    }
    vectors
}

/// Every minor version 0 record vector, with its bytes.
#[must_use]
pub fn minor_0_records() -> Vec<RecordVector> {
    vec![
        RecordVector {
            name: "invocation",
            record: Record::Invocation(CommandInvocation::new(
                ids::RENAME_TAB,
                vec![
                    CommandArgument::new(
                        "name",
                        CommandValue::Text("logs".into()),
                    ),
                    CommandArgument::new("tab", CommandValue::Tab(tab_id(4))),
                ],
            )),
            bytes: INVOCATION,
        },
        RecordVector {
            name: "invocation of every value",
            record: Record::Invocation(CommandInvocation::new(
                ids::SPLIT_RIGHT,
                vec![
                    CommandArgument::new("a", CommandValue::Bool(true)),
                    CommandArgument::new("b", CommandValue::Integer(-3)),
                    CommandArgument::new(
                        "c",
                        CommandValue::Workspace(workspace_id(2)),
                    ),
                    CommandArgument::new(
                        "d",
                        CommandValue::Session(session_id(1)),
                    ),
                    CommandArgument::new("e", CommandValue::Pane(pane_id(5))),
                ],
            )),
            bytes: INVOCATION_VALUES,
        },
        RecordVector {
            name: "completed",
            record: Record::Outcome(CommandOutcome::Completed),
            bytes: &[0],
        },
        RecordVector {
            name: "accepted",
            record: Record::Outcome(CommandOutcome::Accepted),
            bytes: &[1],
        },
        RecordVector {
            name: "busy error",
            record: Record::Error(WireError::new(ErrorCode::Busy, "busy")),
            bytes: ERROR_BUSY,
        },
        RecordVector {
            name: "committed error",
            record: Record::Error(WireError {
                code: ErrorCode::Internal,
                retry: Retry::Never,
                committed_seq: Some(300),
                message: "cleanup failed".into(),
            }),
            bytes: ERROR_COMMITTED,
        },
    ]
}

/// Every framing vector, with its bytes.
#[must_use]
pub fn frames() -> Vec<FrameVector> {
    vec![
        FrameVector {
            name: "final frame",
            frame: Frame {
                stream: 0,
                end: FrameEnd::Final,
                payload: vec![0xaa, 0xbb],
            },
            bytes: &[4, 0, 0, 0, 1, 0, 0xaa, 0xbb],
        },
        FrameVector {
            name: "continued frame",
            frame: Frame {
                stream: 2,
                end: FrameEnd::More,
                payload: vec![0xcc],
            },
            bytes: &[3, 0, 0, 0, 0, 2, 0xcc],
        },
        FrameVector {
            name: "abort",
            frame: Frame {
                stream: 3,
                end: FrameEnd::Abort,
                payload: Vec::new(),
            },
            bytes: &[2, 0, 0, 0, 2, 3],
        },
    ]
}

/// The preamble's bytes at framing version 1.
pub const PREAMBLE: [u8; 8] = *b"HUTRM\0\x01\x00";

const HELLO: &[u8] = &[
    0x00, 0x16, 0x01, 0x00, 0x01, 0x01, 0x00, 0x80, 0x80, 0x40, 0x06, 0x68,
    0x75, 0x74, 0x65, 0x72, 0x6d, 0x06, 0x30, 0x2e, 0x33, 0x31, 0x2e, 0x30,
];
const WELCOME: &[u8] = &[
    0x01, 0x5e, 0x01, 0x00, 0x00, 0x10, 0x32, 0x54, 0x76, 0x98, 0xba, 0xdc,
    0xfe, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xa5, 0xa5, 0xa5,
    0xa5, 0xa5, 0xa5, 0xa5, 0xa5, 0xa5, 0xa5, 0xa5, 0xa5, 0xa5, 0xa5, 0xa5,
    0xa5, 0x06, 0x64, 0x65, 0x76, 0x62, 0x6f, 0x78, 0x07, 0x64, 0x65, 0x66,
    0x61, 0x75, 0x6c, 0x74, 0x06, 0x30, 0x2e, 0x33, 0x31, 0x2e, 0x30, 0x92,
    0x21, 0x80, 0x80, 0x40, 0x1f, 0x40, 0x02, 0x80, 0x80, 0x40, 0xec, 0x82,
    0x80, 0x02, 0x02, 0x00, 0x80, 0x80, 0x01, 0x06, 0xdb, 0x80, 0x40, 0xb0,
    0xea, 0x01, 0xb0, 0xea, 0x01, 0xd0, 0x0f, 0x80, 0x80, 0x80, 0x04, 0x00,
];
const REJECT_MAJOR: &[u8] = &[
    0x02, 0x0c, 0x00, 0x02, 0x01, 0x00, 0x07, 0x75, 0x70, 0x67, 0x72, 0x61,
    0x64, 0x65,
];
const REJECT_BUSY: &[u8] = &[0x02, 0x03, 0x01, 0x00, 0x00];
const REJECT_MALFORMED: &[u8] =
    &[0x02, 0x06, 0x02, 0x00, 0x03, 0x62, 0x61, 0x64];
const HIERARCHY_SNAPSHOT: &[u8] = &[
    0x03, 0x84, 0x01, 0x2a, 0x37, 0xe5, 0x8e, 0xf8, 0x3a, 0xc7, 0x8b, 0x52,
    0x7a, 0x02, 0x62, 0x11, 0x01, 0x01, 0x04, 0x77, 0x6f, 0x72, 0x6b, 0x09,
    0x53, 0x65, 0x73, 0x73, 0x69, 0x6f, 0x6e, 0x20, 0x31, 0x01, 0x4e, 0x0e,
    0x02, 0x00, 0x0b, 0x57, 0x6f, 0x72, 0x6b, 0x73, 0x70, 0x61, 0x63, 0x65,
    0x20, 0x31, 0x02, 0x2f, 0x04, 0x01, 0x05, 0x62, 0x75, 0x69, 0x6c, 0x64,
    0x01, 0x22, 0x09, 0x00, 0x02, 0x11, 0xa0, 0xc2, 0x1e, 0x00, 0x0c, 0x05,
    0x06, 0x03, 0x7a, 0x73, 0x68, 0x01, 0x04, 0x6c, 0x6f, 0x67, 0x73, 0x0c,
    0xa0, 0xc2, 0x1e, 0x00, 0x07, 0x07, 0x08, 0x03, 0x7a, 0x73, 0x68, 0x00,
    0x01, 0x07, 0x01, 0x0d, 0x0a, 0x00, 0x00, 0x07, 0x0b, 0x0c, 0x03, 0x7a,
    0x73, 0x68, 0x00, 0x00, 0x00, 0x0e, 0x0c, 0x03, 0x00, 0x09, 0x53, 0x65,
    0x73, 0x73, 0x69, 0x6f, 0x6e, 0x20, 0x32, 0x00, 0x02, 0x02, 0x0d, 0x01,
    0x02, 0x0e, 0x03,
];
const HIERARCHY_RESYNC: &[u8] = &[
    0x05, 0x84, 0x01, 0x2a, 0x37, 0xe5, 0x8e, 0xf8, 0x3a, 0xc7, 0x8b, 0x52,
    0x7a, 0x02, 0x62, 0x11, 0x01, 0x01, 0x04, 0x77, 0x6f, 0x72, 0x6b, 0x09,
    0x53, 0x65, 0x73, 0x73, 0x69, 0x6f, 0x6e, 0x20, 0x31, 0x01, 0x4e, 0x0e,
    0x02, 0x00, 0x0b, 0x57, 0x6f, 0x72, 0x6b, 0x73, 0x70, 0x61, 0x63, 0x65,
    0x20, 0x31, 0x02, 0x2f, 0x04, 0x01, 0x05, 0x62, 0x75, 0x69, 0x6c, 0x64,
    0x01, 0x22, 0x09, 0x00, 0x02, 0x11, 0xa0, 0xc2, 0x1e, 0x00, 0x0c, 0x05,
    0x06, 0x03, 0x7a, 0x73, 0x68, 0x01, 0x04, 0x6c, 0x6f, 0x67, 0x73, 0x0c,
    0xa0, 0xc2, 0x1e, 0x00, 0x07, 0x07, 0x08, 0x03, 0x7a, 0x73, 0x68, 0x00,
    0x01, 0x07, 0x01, 0x0d, 0x0a, 0x00, 0x00, 0x07, 0x0b, 0x0c, 0x03, 0x7a,
    0x73, 0x68, 0x00, 0x00, 0x00, 0x0e, 0x0c, 0x03, 0x00, 0x09, 0x53, 0x65,
    0x73, 0x73, 0x69, 0x6f, 0x6e, 0x20, 0x32, 0x00, 0x02, 0x02, 0x0d, 0x01,
    0x02, 0x0e, 0x03,
];
const PASTE: &[u8] = &[
    0x06, 0x0b, 0x1e, 0x07, 0x08, 0x65, 0x63, 0x68, 0x6f, 0x20, 0x68, 0x69,
    0x0a,
];
const CLIPBOARD_WRITE: &[u8] = &[
    0x07, 0x0f, 0x01, 0x1e, 0x02, 0x06, 0x00, 0x08, 0x00, 0x06, 0x63, 0x6f,
    0x70, 0x69, 0x65, 0x64, 0x00,
];
const EVENTS: [&[u8]; 21] = [
    &[
        0x04, 0x1a, 0x2b, 0x00, 0x0e, 0x0c, 0x14, 0x00, 0x09, 0x53, 0x65, 0x73,
        0x73, 0x69, 0x6f, 0x6e, 0x20, 0x33, 0x02, 0x01, 0xef, 0xcd, 0xab, 0x89,
        0x67, 0x45, 0x23, 0x01,
    ],
    &[
        0x04, 0x0b, 0x2c, 0x01, 0x07, 0x01, 0x01, 0x04, 0x70, 0x6c, 0x61, 0x79,
        0x00,
    ],
    &[0x04, 0x05, 0x2d, 0x02, 0x01, 0x03, 0x00],
    &[
        0x04, 0x1a, 0x2e, 0x03, 0x16, 0x01, 0x13, 0x15, 0x01, 0x04, 0x64, 0x6f,
        0x63, 0x73, 0x0b, 0x57, 0x6f, 0x72, 0x6b, 0x73, 0x70, 0x61, 0x63, 0x65,
        0x20, 0x32, 0x01, 0x00,
    ],
    &[0x04, 0x06, 0x2f, 0x04, 0x02, 0x02, 0x00, 0x00],
    &[0x04, 0x07, 0x30, 0x05, 0x03, 0x02, 0x03, 0x00, 0x00],
    &[0x04, 0x05, 0x31, 0x06, 0x01, 0x02, 0x00],
    &[
        0x04, 0x14, 0x32, 0x07, 0x10, 0x02, 0x0d, 0x16, 0x00, 0x00, 0x07, 0x17,
        0x18, 0x03, 0x7a, 0x73, 0x68, 0x00, 0x00, 0x00, 0x02, 0x00,
    ],
    &[
        0x04, 0x0c, 0x33, 0x08, 0x08, 0x0a, 0x01, 0x05, 0x74, 0x65, 0x73, 0x74,
        0x73, 0x00,
    ],
    &[
        0x04, 0x0c, 0x34, 0x09, 0x08, 0x07, 0x01, 0x05, 0x73, 0x68, 0x65, 0x6c,
        0x6c, 0x00,
    ],
    &[0x04, 0x07, 0x35, 0x0a, 0x03, 0x0a, 0x02, 0x00, 0x00],
    &[0x04, 0x05, 0x36, 0x0b, 0x01, 0x0a, 0x00],
    &[
        0x04, 0x14, 0x37, 0x0c, 0x10, 0x04, 0x00, 0x0c, 0x05, 0x06, 0x03, 0x7a,
        0x73, 0x68, 0x01, 0x04, 0x6c, 0x6f, 0x67, 0x73, 0x00, 0x00,
    ],
    &[0x04, 0x06, 0x38, 0x0d, 0x02, 0x04, 0x00, 0x00],
    &[0x04, 0x06, 0x39, 0x0e, 0x02, 0x04, 0x00, 0x00],
    &[
        0x04, 0x31, 0x3a, 0x0f, 0x2d, 0x04, 0x00, 0x10, 0x04, 0x00, 0x0c, 0x05,
        0x06, 0x03, 0x7a, 0x73, 0x68, 0x01, 0x04, 0x6c, 0x6f, 0x67, 0x73, 0x00,
        0x02, 0x10, 0x02, 0x0d, 0x18, 0x00, 0x00, 0x07, 0x07, 0x08, 0x03, 0x7a,
        0x73, 0x68, 0x00, 0x00, 0x00, 0x01, 0x01, 0x03, 0x0a, 0x02, 0x02, 0x03,
        0x01, 0x19, 0x00,
    ],
    &[0x04, 0x06, 0x3b, 0x10, 0x02, 0x1a, 0x01, 0x00],
    &[0x04, 0x06, 0x3c, 0x11, 0x02, 0x0d, 0x03, 0x00],
    &[0x04, 0x05, 0x3d, 0x12, 0x01, 0x0e, 0x00],
    &[0x04, 0x04, 0x3e, 0x13, 0x00, 0x00],
    &[0x04, 0x04, 0x3f, 0x14, 0x00, 0x00],
];
const INVOCATION: &[u8] = &[
    0x21, 0x0a, 0x72, 0x65, 0x6e, 0x61, 0x6d, 0x65, 0x5f, 0x74, 0x61, 0x62,
    0x02, 0x0c, 0x04, 0x6e, 0x61, 0x6d, 0x65, 0x02, 0x05, 0x04, 0x6c, 0x6f,
    0x67, 0x73, 0x07, 0x03, 0x74, 0x61, 0x62, 0x03, 0x01, 0x04,
];
const INVOCATION_VALUES: &[u8] = &[
    0x2b, 0x0b, 0x73, 0x70, 0x6c, 0x69, 0x74, 0x5f, 0x72, 0x69, 0x67, 0x68,
    0x74, 0x05, 0x05, 0x01, 0x61, 0x00, 0x01, 0x01, 0x05, 0x01, 0x62, 0x01,
    0x01, 0x05, 0x05, 0x01, 0x63, 0x04, 0x01, 0x02, 0x05, 0x01, 0x64, 0x05,
    0x01, 0x01, 0x05, 0x01, 0x65, 0x06, 0x01, 0x05,
];
const ERROR_BUSY: &[u8] =
    &[0x08, 0x06, 0x00, 0x00, 0x04, 0x62, 0x75, 0x73, 0x79];
const ERROR_COMMITTED: &[u8] = &[
    0x14, 0x13, 0x01, 0x01, 0xac, 0x02, 0x0e, 0x63, 0x6c, 0x65, 0x61, 0x6e,
    0x75, 0x70, 0x20, 0x66, 0x61, 0x69, 0x6c, 0x65, 0x64,
];

#[cfg(test)]
mod tests;
