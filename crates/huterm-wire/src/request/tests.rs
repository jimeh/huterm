use huterm_protocol::{
    CommandArgument, CommandInvocation, CommandValue, PaneInfo, PaneNode,
    SPLIT_TOTAL, SplitChild, SplitNode, TerminalId, ids,
};

use super::*;
use crate::caps::{Direction, MessageCaps, MessageKind};
use crate::command::{MAX_ARGUMENT_TEXT_BYTES, MAX_COMMAND_ARGUMENTS};
use crate::launch::{
    LAUNCH_PAYLOAD_LIMIT, MAX_LAUNCH_ARGUMENTS, MAX_LAUNCH_ENVIRONMENT,
    MAX_LAUNCH_PATH_BYTES,
};
use crate::message::{Message, decode_message, encode_message};
use crate::vectors::{self, RUNTIME};

const CAPS: MessageCaps = MessageCaps::for_minor(0);

fn message(request: Request) -> Message {
    Message::Request(RequestMessage {
        id: RequestId::new(u64::MAX).unwrap(),
        request,
    })
}

fn encode(request: Request) -> Result<Vec<u8>, EncodeError> {
    encode_message(&message(request), vectors::scope(), CAPS)
}

/// Encodes `request`, checks it fits the request cap, and decodes it back.
fn fits(request: Request) {
    let message = message(request);
    let bytes = encode_message(&message, vectors::scope(), CAPS).unwrap();
    let cap = CAPS.cap(MessageKind::Request).unwrap();
    assert!(bytes.len() <= cap, "{} bytes over {cap}", bytes.len());
    assert_eq!(
        decode_message(&bytes, Direction::ClientToServer, vectors::context(0)),
        Ok(message)
    );
}

fn far<T: crate::codec::WireId>(offset: u64) -> T {
    T::scoped(RUNTIME, u64::MAX - offset)
}

/// A launch at every limit: both paths at their longest, every argument
/// and environment string present with the payload spread among them, so
/// each length prefix takes two bytes, and the widest fixed fields.
fn launch_at_limits() -> TerminalCommand {
    let strings = MAX_LAUNCH_ARGUMENTS + 2 * MAX_LAUNCH_ENVIRONMENT;
    let each = LAUNCH_PAYLOAD_LIMIT / strings;
    let mut arguments = vec!["a".repeat(each); MAX_LAUNCH_ARGUMENTS];
    arguments[0] = "a".repeat(each + LAUNCH_PAYLOAD_LIMIT % strings);
    TerminalCommand {
        program: "p".repeat(MAX_LAUNCH_PATH_BYTES).into(),
        arguments,
        working_directory: "d".repeat(MAX_LAUNCH_PATH_BYTES).into(),
        environment: vec![
            ("n".repeat(each), "v".repeat(each));
            MAX_LAUNCH_ENVIRONMENT
        ],
        grid_size: GridSize {
            columns: u16::MAX,
            rows: u16::MAX,
        },
        cell_size: CellSize {
            width: u16::MAX,
            height: u16::MAX,
        },
        scrollback: ScrollbackLimit::MAX
            .with_bytes(ScrollbackLimit::MAX_BYTES)
            .unwrap(),
        ..vectors::sample_command()
    }
}

#[test]
fn a_launch_and_a_name_at_their_limits_fit_and_one_more_is_refused_locally() {
    let request = |name: usize, command: TerminalCommand| {
        Request::CreateWorkspaceWithTab {
            session: far(0),
            name: Some("n".repeat(name)),
            command: Box::new(command),
        }
    };
    fits(request(MAX_REQUEST_NAME_BYTES, launch_at_limits()));
    let mut split = launch_at_limits();
    split.arguments.truncate(1);
    fits(Request::SplitPane {
        workspace: far(0),
        tab: far(1),
        pane: far(2),
        direction: SplitDirection::Down,
        command: Box::new(split),
    });
    let over = |change: fn(&mut TerminalCommand)| {
        let mut command = launch_at_limits();
        change(&mut command);
        encode(request(0, command))
    };
    assert_eq!(
        encode(request(MAX_REQUEST_NAME_BYTES + 1, launch_at_limits())),
        Err(EncodeError::LimitExceeded { field: "name" })
    );
    assert_eq!(
        over(|command| command.arguments[1].push('a')),
        Err(EncodeError::LimitExceeded {
            field: "launch payload"
        })
    );
    assert_eq!(
        over(|command| command.arguments.push(String::new())),
        Err(EncodeError::LimitExceeded { field: "arguments" })
    );
    assert_eq!(
        over(|command| command
            .environment
            .push((String::new(), String::new()))),
        Err(EncodeError::LimitExceeded {
            field: "environment"
        })
    );
    assert_eq!(
        over(|command| command.program.push("x")),
        Err(EncodeError::LimitExceeded { field: "program" })
    );
}

/// A layout of exactly `items` panes and splits, every pane name at its
/// limit: a root split of four splits that share the panes.
fn layout_of(items: usize) -> PaneLayout {
    let panes = items - 5;
    let mut next = 0;
    let mut id = || {
        next += 1;
        u64::MAX - next
    };
    let split = |id: u64, children: Vec<PaneNode>| {
        let count = u32::try_from(children.len()).unwrap();
        PaneNode::Split(SplitNode {
            id: SplitId::in_runtime(RUNTIME, id),
            axis: SplitAxis::Columns,
            children: children
                .into_iter()
                .enumerate()
                .map(|(index, node)| SplitChild {
                    weight: SPLIT_TOTAL / count
                        + if index == 0 { SPLIT_TOTAL % count } else { 0 },
                    node,
                })
                .collect(),
        })
    };
    let groups = (0..4)
        .map(|group| {
            let count = panes / 4 + usize::from(group < panes % 4);
            let children = (0..count)
                .map(|_| {
                    let value = id();
                    PaneNode::Pane(PaneInfo {
                        id: PaneId::in_runtime(RUNTIME, value),
                        terminal_id: TerminalId::in_runtime(RUNTIME, value),
                        fallback_name: "f".repeat(MAX_NAME_BYTES),
                        custom_name: Some("c".repeat(MAX_NAME_BYTES)),
                    })
                })
                .collect();
            split(id(), children)
        })
        .collect();
    let layout = PaneLayout::new(split(id(), groups)).unwrap();
    assert_eq!(layout.pane_count() + layout.split_ids().len(), items);
    layout
}

fn arrangement_of(node: &PaneNode) -> Arrangement {
    match node {
        PaneNode::Pane(pane) => Arrangement::Pane(pane.id),
        PaneNode::Split(split) => Arrangement::Split {
            axis: split.axis,
            children: split
                .children
                .iter()
                .map(|child| (child.weight, arrangement_of(&child.node)))
                .collect(),
        },
    }
}

#[test]
fn an_arrangement_at_its_limits_fits_and_one_more_item_is_refused_locally() {
    let arrange = |expected: PaneLayout, arrangement: Arrangement| {
        Request::ArrangePanes {
            slot: u32::MAX,
            workspace: far(0),
            tab: far(1),
            expected,
            arrangement,
        }
    };
    let at_limit = layout_of(MAX_REQUEST_LAYOUT_ITEMS);
    let over = layout_of(MAX_REQUEST_LAYOUT_ITEMS + 1);
    let arrangement = arrangement_of(at_limit.root());
    fits(arrange(at_limit.clone(), arrangement.clone()));
    assert_eq!(
        encode(arrange(over.clone(), arrangement.clone())),
        Err(EncodeError::LimitExceeded { field: "layout" })
    );
    assert_eq!(
        encode(arrange(at_limit, arrangement_of(over.root()))),
        Err(EncodeError::LimitExceeded {
            field: "arrangement"
        })
    );
}

#[test]
fn an_invocation_and_a_close_of_many_tabs_at_their_limits_fit() {
    let name = |index: usize| format!("{index:0>64}");
    fits(Request::ExecuteCommand {
        invocation: DecodedInvocation::Known(CommandInvocation::new(
            ids::RENAME_TAB,
            (0..MAX_COMMAND_ARGUMENTS)
                .map(|index| {
                    CommandArgument::new(
                        name(index),
                        CommandValue::Text("t".repeat(MAX_ARGUMENT_TEXT_BYTES)),
                    )
                })
                .collect(),
        )),
    });
    let tabs = |count: u64| CloseRequest::Tabs {
        workspace: far(0),
        tabs: (1..=count).map(far).collect(),
    };
    let limit = u64::try_from(MAX_CLOSE_REQUEST_TABS).unwrap();
    fits(Request::PrepareClose {
        request: tabs(limit),
    });
    assert_eq!(
        encode(Request::PrepareClose {
            request: tabs(limit + 1)
        }),
        Err(EncodeError::LimitExceeded { field: "tabs" })
    );
}

#[test]
fn requests_and_their_kinds_carry_their_retry_class_and_version() {
    let numbers: Vec<u64> =
        RequestKind::ALL.iter().map(|kind| kind.number()).collect();
    assert_eq!(
        numbers,
        (0..u64::try_from(RequestKind::ALL.len()).unwrap()).collect::<Vec<_>>()
    );
    for kind in RequestKind::ALL {
        assert_eq!(RequestKind::from_number(kind.number()), Some(*kind));
        assert_eq!(
            kind.large_result(),
            *kind == RequestKind::Selection,
            "{kind:?}"
        );
        // Every request that names a viewer is a viewer request or a
        // shared setter, which is a mutation.
        if kind.names_viewer() {
            assert_ne!(kind.retry_class(), RetryClass::Query, "{kind:?}");
        }
    }
    let class = |class| {
        RequestKind::ALL
            .iter()
            .filter(|kind| kind.retry_class() == class)
            .copied()
            .collect::<Vec<_>>()
    };
    assert_eq!(
        class(RetryClass::Query),
        [
            RequestKind::SubscribeHierarchy,
            RequestKind::ResyncHierarchy,
            RequestKind::SelectSession,
            RequestKind::SelectWorkspace,
            RequestKind::SelectTab,
            RequestKind::SelectPane,
            RequestKind::PrepareClose,
        ]
    );
    assert_eq!(
        class(RetryClass::Viewer),
        [
            RequestKind::ReportGeometry,
            RequestKind::SubscribeTerminal,
            RequestKind::CloseViewer,
            RequestKind::Snapshot,
            RequestKind::Selection,
            RequestKind::SelectUnits,
            RequestKind::UpdatePresentation,
        ]
    );
    // Shared setters are mutations, never retried.
    for kind in [
        RequestKind::SetScrollback,
        RequestKind::SetScrollbackCompression,
        RequestKind::ClearHistory,
        RequestKind::Reset,
        RequestKind::RenameTab,
        RequestKind::ToggleInputSync,
    ] {
        assert_eq!(kind.retry_class(), RetryClass::Mutation);
    }
    for request in vectors::every_request() {
        assert_eq!(request.kind().since(), 0, "{request:?}");
    }
}

#[test]
fn a_zero_request_identity_is_a_typed_error_in_every_kind_that_names_one() {
    let mut request = Writer::new();
    request
        .record(|w| {
            w.u64(0);
            w.tag(RequestKind::SubscribeHierarchy.number());
            w.record(|_| Ok(()))
        })
        .unwrap();
    assert_eq!(
        decode_request(
            &mut Reader::new(&request.into_bytes()),
            vectors::scope(),
            0
        ),
        Err(DecodeError::Invalid("request ID"))
    );
    let zero = [1, 0];
    assert_eq!(
        decode_cancel(&mut Reader::new(&zero)),
        Err(DecodeError::Invalid("request ID"))
    );
    assert_eq!(
        crate::response::decode_result(
            &mut Reader::new(&zero),
            vectors::scope()
        ),
        Err(DecodeError::Invalid("request ID"))
    );
    assert_eq!(RequestId::new(0), None);
    assert_eq!(RequestId::FIRST.next().map(RequestId::get), Some(2));
}

#[test]
fn unknown_request_and_close_tags_are_typed_errors() {
    let mut w = Writer::new();
    w.record(|w| {
        w.u64(1);
        w.tag(999);
        w.record(|_| Ok(()))
    })
    .unwrap();
    assert_eq!(
        decode_request(&mut Reader::new(&w.into_bytes()), vectors::scope(), 0),
        Err(DecodeError::UnknownTag {
            field: "request",
            tag: 999
        })
    );
    let unknown_close = [8, 0];
    assert_eq!(
        decode_close_request(
            &mut Reader::new(&unknown_close),
            vectors::scope()
        ),
        Err(DecodeError::UnknownTag {
            field: "close request",
            tag: 8
        })
    );
}

/// An arrangement of `depth` nested single-child splits around a pane,
/// written by hand because no valid arrangement nests that deep.
fn nested_arrangement(depth: usize) -> Vec<u8> {
    fn node(w: &mut Writer, depth: usize) -> Result<(), EncodeError> {
        if depth == 1 {
            w.tag(0);
            return w.record(|w| {
                w.u64(1);
                Ok(())
            });
        }
        w.tag(1);
        w.record(|w| {
            w.tag(0);
            w.u64(1);
            w.record(|w| {
                w.u32(SPLIT_TOTAL);
                node(w, depth - 1)
            })
        })
    }
    let mut w = Writer::new();
    node(&mut w, depth).unwrap();
    w.into_bytes()
}

#[test]
fn arrangements_deeper_than_the_layout_depth_are_refused_both_ways() {
    let scope = vectors::scope();
    let at_limit = nested_arrangement(MAX_LAYOUT_DEPTH);
    let decoded = decode_arrangement(&mut Reader::new(&at_limit), scope);
    let decoded = decoded.unwrap();
    let too_deep = Arrangement::Split {
        axis: SplitAxis::Columns,
        children: vec![(SPLIT_TOTAL, decoded)],
    };
    assert_eq!(
        decode_arrangement(
            &mut Reader::new(&nested_arrangement(MAX_LAYOUT_DEPTH + 1)),
            scope
        ),
        Err(DecodeError::Invalid("arrangement depth"))
    );
    assert_eq!(
        encode_arrangement(&mut Writer::new(), scope, &too_deep),
        Err(EncodeError::Invalid("arrangement depth"))
    );
}
