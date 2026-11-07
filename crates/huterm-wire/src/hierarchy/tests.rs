use std::time::Duration;

use huterm_protocol::{
    SPLIT_TOTAL, SplitDirection, SplitId, TerminalId, WorkspaceId,
};
use proptest::prelude::*;

use super::*;
use crate::caps::{Direction, MessageCaps, MessageKind};
use crate::frame::{FrameDecoder, MIN_MAX_FRAME};
use crate::message::{Message, decode_message, encode_message};
use crate::stream::{OutboundMessage, Reassembler};
use crate::vectors::{self, RUNTIME};

const CAPS: MessageCaps = MessageCaps::for_minor(0);
/// Identities from here up take a varint's full ten bytes.
const WIDE: u64 = 1 << 63;

fn name(fill: char) -> String {
    fill.to_string().repeat(MAX_NAME_BYTES)
}

fn session(value: u64) -> SessionInfo {
    SessionInfo {
        id: SessionId::in_runtime(RUNTIME, WIDE + value),
        custom_name: Some(name('s')),
        automatic_name: name('a'),
    }
}

fn workspace(value: u64) -> WorkspaceInfo {
    WorkspaceInfo {
        id: WorkspaceId::in_runtime(RUNTIME, WIDE + value),
        custom_name: Some(name('w')),
        automatic_name: name('a'),
    }
}

fn pane(value: u64) -> PaneInfo {
    PaneInfo {
        id: PaneId::in_runtime(RUNTIME, WIDE + value),
        terminal_id: TerminalId::in_runtime(RUNTIME, WIDE + value),
        fallback_name: name('f'),
        custom_name: Some(name('p')),
    }
}

fn tab(value: u64, layout: PaneLayout) -> TabInfo {
    TabInfo {
        id: TabId::in_runtime(RUNTIME, WIDE + value),
        custom_name: Some(name('t')),
        zoomed: (layout.pane_count() > 1).then(|| layout.first_pane().id),
        layout,
        input_sync: true,
    }
}

/// `count` panes side by side under one split.
fn wide_split(first: u64, count: u64) -> PaneLayout {
    let weight = SPLIT_TOTAL / u32::try_from(count).unwrap();
    let mut weights = vec![weight; usize::try_from(count).unwrap()];
    weights[0] += SPLIT_TOTAL - weight * u32::try_from(count).unwrap();
    PaneLayout::new(PaneNode::Split(SplitNode {
        id: SplitId::in_runtime(RUNTIME, WIDE + first),
        axis: SplitAxis::Rows,
        children: weights
            .into_iter()
            .zip(first + 1..)
            .map(|(weight, value)| SplitChild {
                weight,
                node: PaneNode::Pane(pane(value)),
            })
            .collect(),
    }))
    .unwrap()
}

fn stream() -> StreamId {
    vectors::context(0).hierarchy.stream
}

/// The three densest shapes a full hierarchy takes: only sessions; one
/// workspace of one-pane tabs; and tabs of full splits.
fn full_hierarchies() -> Vec<HierarchyState> {
    let only_sessions = (0..MAX_HIERARCHY_ITEMS as u64).map(|value| {
        (session(value), Vec::<(WorkspaceInfo, Vec<TabInfo>)>::new())
    });
    // A session and a workspace, then a tab and its pane per two items.
    let single_tabs: Vec<TabInfo> = (0..(MAX_HIERARCHY_ITEMS as u64 - 2) / 2)
        .map(|value| tab(2 * value, PaneLayout::single(pane(2 * value + 1))))
        .collect();
    // A tab, a split, and 254 panes per 256 items.
    let split_tabs: Vec<TabInfo> = (0..(MAX_HIERARCHY_ITEMS as u64 - 2) / 256)
        .map(|value| tab(256 * value, wide_split(256 * value + 1, 254)))
        .collect();
    [
        HierarchyState::from_sessions(stream(), 9, only_sessions, []),
        HierarchyState::from_sessions(
            stream(),
            9,
            [(
                session(u64::MAX >> 1),
                vec![(workspace(u64::MAX >> 1), single_tabs)],
            )],
            [],
        ),
        HierarchyState::from_sessions(
            stream(),
            9,
            [(
                session(u64::MAX >> 1),
                vec![(workspace(u64::MAX >> 1), split_tabs)],
            )],
            [],
        ),
    ]
    .into_iter()
    .map(Option::unwrap)
    .collect()
}

fn items(state: &HierarchyState) -> usize {
    let mut items = state.sessions().len();
    for session in state.sessions() {
        for workspace in state.session_workspaces(*session).unwrap() {
            items += 1;
            for tab in state.workspace_tabs(*workspace).unwrap() {
                let layout = &state.tab(*tab).unwrap().layout;
                items += 1 + layout.pane_count() + layout.split_ids().len();
            }
        }
    }
    items
}

/// Sends `bytes` from the server in frames of at most `max_frame` and
/// reassembles them on the client.
fn across_frames(bytes: &[u8], max_frame: u32) -> Vec<u8> {
    let mut outbound = OutboundMessage::new(0, bytes.to_vec());
    let mut decoder = FrameDecoder::new();
    decoder.set_limit(max_frame);
    while let Some(frame) = outbound.next_frame(max_frame) {
        decoder.push(&frame.encode(max_frame).unwrap());
    }
    let mut receiver =
        Reassembler::new(CAPS, Direction::ServerToClient, usize::MAX);
    let mut assembled = None;
    while let Some(frame) = decoder.next_frame().unwrap() {
        assembled = receiver
            .accept(frame, Duration::ZERO)
            .unwrap()
            .or(assembled);
    }
    assembled.unwrap().bytes
}

#[test]
fn a_full_hierarchy_with_every_name_at_its_limit_fits_its_cap_at_the_smallest_frames()
 {
    let cap = CAPS.cap(MessageKind::HierarchySnapshot).unwrap();
    for state in full_hierarchies() {
        assert!(items(&state) <= MAX_HIERARCHY_ITEMS);
        assert!(items(&state) > MAX_HIERARCHY_ITEMS - 256);
        let message = Message::HierarchySnapshot(HierarchySnapshot {
            digest: state.digest(),
            state,
        });
        let bytes = encode_message(&message, vectors::scope(), CAPS).unwrap();
        assert!(bytes.len() <= cap, "{} > {cap}", bytes.len());
        let received = across_frames(&bytes, MIN_MAX_FRAME);
        assert_eq!(
            decode_message(
                &received,
                Direction::ServerToClient,
                vectors::context(0)
            ),
            Ok(message)
        );
    }
}

#[test]
fn a_hierarchy_past_the_item_limit_does_not_decode() {
    // Every session sequence fits its count limit; the attachment is one
    // item too many.
    let sessions = (0..MAX_HIERARCHY_ITEMS as u64).map(|value| {
        (session(value), Vec::<(WorkspaceInfo, Vec<TabInfo>)>::new())
    });
    let attachment = (AttachmentId::in_runtime(RUNTIME, 1), session(0).id);
    let state =
        HierarchyState::from_sessions(stream(), 1, sessions, [attachment])
            .unwrap();
    let mut w = Writer::new();
    encode_snapshot(
        &mut w,
        vectors::scope(),
        &HierarchySnapshot { digest: 0, state },
    )
    .unwrap();
    let bytes = w.into_bytes();
    assert_eq!(
        decode_snapshot(
            &mut Reader::new(&bytes),
            vectors::context(0).hierarchy
        ),
        Err(DecodeError::Invalid("hierarchy item count"))
    );
}

/// The bytes of `depth` nested splits around one pane, built from the
/// inside out so no layout of that depth ever exists.
fn nested_bytes(depth: usize) -> Vec<u8> {
    let scope = vectors::scope();
    let mut node = Writer::new();
    encode_node(&mut node, scope, &PaneNode::Pane(pane(0))).unwrap();
    let mut node = node.into_bytes();
    for level in 1..depth as u64 {
        let mut split = Writer::new();
        split.tag(1);
        split
            .record(|w| {
                w.id(scope, SplitId::in_runtime(RUNTIME, level))?;
                w.tag(level % 2);
                w.usize(2);
                for child in [Some(level), None] {
                    w.record(|w| {
                        w.u32(SPLIT_TOTAL / 2);
                        if let Some(value) = child {
                            encode_node(
                                w,
                                scope,
                                &PaneNode::Pane(pane(1000 + value)),
                            )
                        } else {
                            w.fixed(&node);
                            Ok(())
                        }
                    })?;
                }
                Ok(())
            })
            .unwrap();
        node = split.into_bytes();
    }
    node
}

#[test]
fn layouts_deeper_than_the_depth_limit_are_refused_before_recursing() {
    let decode =
        |bytes: &[u8]| decode_layout(&mut Reader::new(bytes), vectors::scope());
    assert_eq!(
        decode(&nested_bytes(MAX_LAYOUT_DEPTH)).unwrap().depth(),
        MAX_LAYOUT_DEPTH
    );
    assert_eq!(
        decode(&nested_bytes(MAX_LAYOUT_DEPTH + 1)),
        Err(DecodeError::Invalid("pane layout depth"))
    );
    // Far deeper input would exhaust the stack if the decoder recursed
    // into it.
    assert_eq!(
        decode(&nested_bytes(2_000)),
        Err(DecodeError::Invalid("pane layout depth"))
    );
    // A layout that decodes but breaks an invariant is refused through the
    // protocol's constructor.
    let mut repeated = Writer::new();
    repeated.tag(1);
    repeated
        .record(|w| {
            w.id(vectors::scope(), SplitId::in_runtime(RUNTIME, 1))?;
            w.tag(0);
            w.usize(2);
            for _ in 0..2 {
                w.record(|w| {
                    w.u32(SPLIT_TOTAL / 2);
                    encode_node(w, vectors::scope(), &PaneNode::Pane(pane(5)))
                })?;
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(
        decode(&repeated.into_bytes()),
        Err(DecodeError::Invalid("pane layout"))
    );
}

/// A layout grown by random splits, with random names.
fn layouts() -> impl Strategy<Value = PaneLayout> {
    let names = proptest::option::of("[a-z ]{1,12}");
    proptest::collection::vec((0..64_usize, any::<bool>(), names), 0..24)
        .prop_map(|steps| {
            let mut layout = PaneLayout::single(pane(0));
            for (index, (target, down, name)) in steps.into_iter().enumerate() {
                let panes = layout.panes();
                let target = panes[target % panes.len()].id;
                let value = 2 * index as u64 + 1;
                let direction = if down {
                    SplitDirection::Down
                } else {
                    SplitDirection::Right
                };
                let added = PaneInfo {
                    custom_name: name,
                    ..pane(value)
                };
                let split = SplitId::in_runtime(RUNTIME, value + 1);
                if let Ok(grown) =
                    layout.with_split(target, direction, added, split)
                {
                    layout = grown;
                }
            }
            layout
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn layouts_and_the_events_that_carry_them_round_trip(
        layout in layouts(),
        name in proptest::option::of(".{0,40}"),
        index: u32,
    ) {
        let scope = vectors::scope();
        let mut w = Writer::new();
        encode_layout(&mut w, scope, &layout).unwrap();
        let bytes = w.into_bytes();
        prop_assert_eq!(decode_layout(&mut Reader::new(&bytes), scope), Ok(layout.clone()));
        let tab = tab(9, layout.clone());
        let events = [
            HierarchyEvent::TabOpened {
                workspace: WorkspaceId::in_runtime(RUNTIME, 3),
                tab: tab.clone(),
                index,
            },
            HierarchyEvent::PaneLayoutChanged {
                tab: tab.id,
                layout: layout.clone(),
                zoomed: tab.zoomed,
            },
            HierarchyEvent::PanesRearranged {
                changes: vec![
                    TabChange::Layout { tab: tab.id, layout, zoomed: None },
                    TabChange::Placed {
                        tab: tab.id,
                        workspace: WorkspaceId::in_runtime(RUNTIME, 3),
                        index,
                    },
                ],
            },
            HierarchyEvent::TabRenamed {
                tab: tab.id,
                custom_name: name,
            },
        ];
        for event in events {
            let mut w = Writer::new();
            encode_event(&mut w, scope, &event).unwrap();
            let bytes = w.into_bytes();
            let mut r = Reader::new(&bytes);
            prop_assert_eq!(decode_event(&mut r, scope), Ok(event));
            prop_assert!(r.is_empty());
        }
    }

    #[test]
    fn arbitrary_bytes_never_panic_the_hierarchy_decoders(
        bytes in proptest::collection::vec(any::<u8>(), 0..256),
    ) {
        let context = vectors::context(0).hierarchy;
        let _ = decode_snapshot(&mut Reader::new(&bytes), context);
        let _ = decode_sequenced(&mut Reader::new(&bytes), context.scope());
        let _ = decode_layout(&mut Reader::new(&bytes), context.scope());
    }
}
