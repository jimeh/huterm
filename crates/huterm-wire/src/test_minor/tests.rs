use huterm_protocol::{
    AttachmentId, PaneInfo, PaneLayout, SessionId, SessionInfo, TerminalId,
    WorkspaceId, WorkspaceInfo,
};

use super::*;
use crate::caps::{Direction, MessageCaps, MessageKind};
use crate::hierarchy::{
    HierarchySnapshot, SequencedEvent, decode_snapshot as decode_minor_0,
};
use crate::message::{Message, decode_message, encode_message};
use crate::vectors::{self, RUNTIME, sample_hierarchy};

fn tab_id(value: u64) -> TabId {
    TabId::in_runtime(RUNTIME, value)
}

fn snapshot_bytes(hierarchy: &TestHierarchy) -> Vec<u8> {
    let mut w = Writer::new();
    encode_snapshot(&mut w, vectors::scope(), hierarchy).unwrap();
    w.into_bytes()
}

#[test]
fn each_minor_reads_the_others_records_skipping_or_defaulting_the_appended_field()
 {
    let mut newer = TestHierarchy::new(sample_hierarchy());
    newer.pinned.insert(tab_id(10));
    let context = vectors::context(0).hierarchy;
    // A minor 0 decoder skips the appended field.
    let bytes = snapshot_bytes(&newer);
    let older = decode_minor_0(&mut Reader::new(&bytes), context).unwrap();
    assert_eq!(older.state, newer.project());
    // A test-minor decoder reads a minor 0 record with the field's default.
    let older_bytes = {
        let mut w = Writer::new();
        crate::hierarchy::encode_snapshot(
            &mut w,
            vectors::scope(),
            &HierarchySnapshot {
                digest: newer.digest(0),
                state: newer.project(),
            },
        )
        .unwrap();
        w.into_bytes()
    };
    let (_, read) =
        decode_snapshot(&mut Reader::new(&older_bytes), context).unwrap();
    assert_eq!(read, TestHierarchy::new(sample_hierarchy()));
    let (digest, read) =
        decode_snapshot(&mut Reader::new(&bytes), context).unwrap();
    assert_eq!(read, newer);
    assert_eq!(digest, newer.digest(TEST_MINOR));
    assert_ne!(newer.digest(TEST_MINOR), newer.digest(0));
}

#[test]
fn a_record_cut_short_before_a_minor_0_field_is_rejected() {
    // A one-pane tab record whose last minor 0 field, input sync, is cut.
    let mut w = Writer::new();
    w.tag(7);
    w.record(|w| {
        w.u64(2);
        w.record(|w| {
            w.u64(10);
            // No custom name.
            w.tag(0);
            crate::hierarchy::encode_layout(
                w,
                vectors::scope(),
                &PaneLayout::single(PaneInfo {
                    id: huterm_protocol::PaneId::in_runtime(RUNTIME, 11),
                    terminal_id: TerminalId::in_runtime(RUNTIME, 12),
                    fallback_name: "sh".into(),
                    custom_name: None,
                }),
            )?;
            // No zoom; the input sync flag is missing.
            w.tag(0);
            Ok(())
        })?;
        w.u32(0);
        Ok(())
    })
    .unwrap();
    let bytes = w.into_bytes();
    let event = decode_event(&mut Reader::new(&bytes), vectors::scope());
    assert_eq!(event, Err(DecodeError::Truncated));
}

/// One session and workspace holding one-pane tabs up to the item limit,
/// with every name at its limit, wide identities, and every tab pinned.
fn full() -> TestHierarchy {
    let wide = 1_u64 << 63;
    let name = |fill: char| fill.to_string().repeat(MAX_NAME_BYTES);
    let tabs: Vec<TabInfo> = (0..(huterm_protocol::MAX_HIERARCHY_ITEMS as u64
        - 2)
        / 2)
        .map(|value| TabInfo {
            id: TabId::in_runtime(RUNTIME, wide + value),
            custom_name: Some(name('t')),
            layout: PaneLayout::single(PaneInfo {
                id: huterm_protocol::PaneId::in_runtime(RUNTIME, wide + value),
                terminal_id: TerminalId::in_runtime(RUNTIME, wide + value),
                fallback_name: name('f'),
                custom_name: Some(name('p')),
            }),
            zoomed: None,
            input_sync: true,
        })
        .collect();
    let pinned = tabs.iter().map(|tab| tab.id).collect();
    let state = HierarchyState::from_sessions(
        vectors::context(0).hierarchy.stream,
        1,
        [(
            SessionInfo {
                id: SessionId::in_runtime(RUNTIME, u64::MAX),
                custom_name: Some(name('s')),
                automatic_name: name('a'),
            },
            vec![(
                WorkspaceInfo {
                    id: WorkspaceId::in_runtime(RUNTIME, u64::MAX),
                    custom_name: Some(name('w')),
                    automatic_name: name('a'),
                },
                tabs,
            )],
        )],
        [(
            AttachmentId::in_runtime(RUNTIME, 1),
            SessionId::in_runtime(RUNTIME, u64::MAX),
        )],
    )
    .unwrap();
    TestHierarchy { state, pinned }
}

#[test]
fn the_test_minors_worst_cases_fit_within_minor_0_caps() {
    let cap = MessageCaps::for_minor(0)
        .cap(MessageKind::HierarchySnapshot)
        .unwrap();
    let hierarchy = full();
    let older = encode_message(
        &Message::HierarchySnapshot(HierarchySnapshot {
            digest: hierarchy.digest(0),
            state: hierarchy.project(),
        }),
        vectors::scope(),
        MessageCaps::for_minor(0),
    )
    .unwrap();
    // The kind and record length prefix the test-minor record, as they do
    // the minor 0 one.
    let newer = snapshot_bytes(&hierarchy);
    // Each pinned tab's appended byte lands inside the record headroom.
    assert!(newer.len() > older.len());
    assert!(newer.len() + 4 <= cap, "{} > {cap}", newer.len());
    for kind in MessageKind::ALL {
        assert!(
            MessageCaps::for_minor(TEST_MINOR).cap(kind)
                <= MessageCaps::for_minor(0).cap(kind),
            "{kind:?}"
        );
    }
}

/// A minor 0 client's copy of the stream, fed only through minor 0 bytes.
struct OlderClient {
    state: HierarchyState,
    skipped: usize,
    equivalents: usize,
    resyncs: usize,
}

impl OlderClient {
    fn receive(&mut self, bytes: &[u8]) {
        let context = vectors::context(0);
        match decode_message(bytes, Direction::ServerToClient, context).unwrap()
        {
            Message::HierarchyEvent(SequencedEvent { seq, event, digest }) => {
                if event == HierarchyEvent::Skipped {
                    self.skipped += 1;
                } else {
                    self.equivalents += 1;
                }
                let envelope = HierarchyEnvelope {
                    stream: self.state.stream(),
                    seq,
                    event,
                };
                assert!(
                    matches!(
                        self.state.apply(envelope),
                        ApplyOutcome::Applied(_)
                    ),
                    "the client never needs a resync of its own"
                );
                assert_eq!(digest, Some(self.state.digest()));
            }
            Message::HierarchyResync(HierarchySnapshot { digest, state }) => {
                self.resyncs += 1;
                assert_eq!(state.seq(), self.state.seq() + 1);
                assert_eq!(digest, state.digest());
                self.state = state;
            }
            message => panic!("unexpected {message:?}"),
        }
    }
}

#[test]
fn a_test_minor_server_converges_with_older_and_newer_clients_through_every_downgrade_path()
 {
    let caps = MessageCaps::for_minor(0);
    let scope = vectors::scope();
    let mut server = TestHierarchy::new(sample_hierarchy());
    let mut older = OlderClient {
        state: sample_hierarchy(),
        skipped: 0,
        equivalents: 0,
        resyncs: 0,
    };
    let mut newer = TestHierarchy::new(sample_hierarchy());
    let events = [
        TestEvent::TabPinned {
            tab: tab_id(4),
            pinned: true,
        },
        TestEvent::TabRetitled {
            tab: tab_id(10),
            custom_name: Some("tests".into()),
        },
        TestEvent::TabsExchanged {
            first: tab_id(4),
            second: tab_id(10),
        },
        TestEvent::Base(HierarchyEvent::TabInputSyncChanged {
            tab: tab_id(4),
            input_sync: false,
        }),
        TestEvent::TabPinned {
            tab: tab_id(10),
            pinned: false,
        },
    ];
    for event in events {
        let mut copy = server.clone();
        let native = copy.apply(&event, TEST_MINOR).unwrap();
        assert_eq!(native, Delivery::Native(event.clone()));
        let delivery = server.apply(&event, 0).unwrap();
        assert_eq!(copy, server);
        let seq = server.state.seq();
        let message = match delivery {
            Delivery::Event(event) => Message::HierarchyEvent(SequencedEvent {
                seq,
                event,
                digest: Some(server.digest(0)),
            }),
            Delivery::Resync(state) => {
                Message::HierarchyResync(HierarchySnapshot {
                    digest: server.digest(0),
                    state,
                })
            }
            Delivery::Native(_) => {
                panic!("a minor 0 peer never gets a native event")
            }
        };
        older.receive(&encode_message(&message, scope, caps).unwrap());
        // The newer client decodes the native event and applies it alike.
        let mut w = Writer::new();
        encode_test_event(&mut w, scope, &event).unwrap();
        let bytes = w.into_bytes();
        let decoded =
            decode_test_event(&mut Reader::new(&bytes), scope).unwrap();
        assert_eq!(decoded, event);
        newer.apply(&decoded, TEST_MINOR).unwrap();
    }
    assert_eq!(older.state, server.project());
    assert_eq!(older.state.digest(), server.digest(0));
    assert_eq!(newer, server);
    assert_eq!(newer.digest(TEST_MINOR), server.digest(TEST_MINOR));
    assert_eq!((older.skipped, older.equivalents, older.resyncs), (2, 2, 1));
    // A test-minor event never decodes at minor 0.
    let mut w = Writer::new();
    encode_test_event(
        &mut w,
        scope,
        &TestEvent::TabPinned {
            tab: tab_id(4),
            pinned: true,
        },
    )
    .unwrap();
    let bytes = w.into_bytes();
    assert!(matches!(
        decode_event(&mut Reader::new(&bytes), scope),
        Err(DecodeError::UnknownTag { tag: 22, .. })
    ));
}

#[test]
fn closing_a_pinned_tabs_workspace_drops_its_pin_so_snapshots_agree() {
    let mut server = TestHierarchy::new(sample_hierarchy());
    let pin = TestEvent::TabPinned {
        tab: tab_id(4),
        pinned: true,
    };
    server.apply(&pin, TEST_MINOR).unwrap();
    let close = TestEvent::Base(HierarchyEvent::WorkspaceClosed {
        workspace: WorkspaceId::in_runtime(RUNTIME, 2),
    });
    server.apply(&close, TEST_MINOR).unwrap();
    assert_eq!(server.pinned, std::collections::BTreeSet::new());
    // A client that installs the server's snapshot computes its digest.
    let (digest, installed) = decode_snapshot(
        &mut Reader::new(&snapshot_bytes(&server)),
        vectors::context(0).hierarchy,
    )
    .unwrap();
    assert_eq!(installed, server);
    assert_eq!(digest, installed.digest(TEST_MINOR));
}
