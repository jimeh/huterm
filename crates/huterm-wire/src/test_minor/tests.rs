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
                    pinned: false,
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
        Err(DecodeError::UnknownTag { tag: 23, .. })
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

mod updates {
    use std::sync::Arc;

    use huterm_protocol::{
        Cell, CellColor, CellStyle, CellText, GridSize, TerminalRow,
        TerminalSnapshot,
    };

    use super::super::*;
    use crate::caps::{Direction, MessageCaps};
    use crate::message::{Message, decode_message, encode_message};
    use crate::update::{
        TerminalUpdate, UPDATE_DIGEST_INTERVAL, UpdateEncoder, UpdateOutcome,
        ViewMetadata, ViewerView, decode_update, encode_update, view_digest,
    };
    use crate::vectors;

    fn cell(text: &str) -> Cell {
        Cell {
            text: CellText::from(text),
            foreground: CellColor::DefaultForeground,
            background: CellColor::DefaultBackground,
            style: CellStyle::default(),
        }
    }

    fn test_row(text: &str, curly: [bool; 2]) -> Arc<TestRow> {
        Arc::new(TestRow {
            row: TerminalRow {
                cells: text.chars().map(|c| cell(&c.to_string())).collect(),
                ..TerminalRow::default()
            },
            curly: curly.to_vec(),
        })
    }

    fn metadata(revision: u64) -> ViewMetadata {
        let mut metadata = ViewMetadata::of(&vectors::sample_snapshot());
        metadata.size = GridSize {
            columns: 2,
            rows: 2,
        };
        metadata.cursor = None;
        metadata.revision = revision;
        metadata
    }

    /// A test-minor viewport whose rows change with `step`, some cells
    /// curly underlined.
    fn viewport(step: u64, kept: &Arc<TestRow>) -> Arc<TestSnapshot> {
        let curly = [step.is_multiple_of(2), step.is_multiple_of(3)];
        Arc::new(TestSnapshot {
            metadata: metadata(step),
            rows: vec![Arc::clone(kept), test_row("ab", curly)],
        })
    }

    /// Sends `update` as minor version 0 bytes, which a minor 0 decoder
    /// must accept.
    fn minor_0_wire(update: &TerminalUpdate) -> TerminalUpdate {
        let bytes = encode_message(
            &Message::TerminalUpdate(update.clone()),
            vectors::scope(),
            MessageCaps::for_minor(0),
        )
        .unwrap();
        match decode_message(
            &bytes,
            Direction::ServerToClient,
            vectors::context(0),
        ) {
            Ok(Message::TerminalUpdate(update)) => update,
            other => panic!("decoded {other:?}"),
        }
    }

    #[test]
    fn a_curly_underline_reaches_a_minor_0_client_as_an_underline_without_a_resync()
     {
        let kept = test_row("$ ", [true, false]);
        let mut server: UpdateEncoder<Arc<TestSnapshot>> =
            UpdateEncoder::new(vectors::viewer(), 0);
        let mut client: ViewerView = ViewerView::new(0);
        let mut digests = 0;
        for step in 0..=UPDATE_DIGEST_INTERVAL + 1 {
            let update = server.next(&viewport(step, &kept), true).unwrap();
            digests += usize::from(update.digest.is_some());
            let disposal = client.apply(&minor_0_wire(&update)).unwrap();
            assert_eq!(disposal.outcome, UpdateOutcome::Applied, "{step}");
        }
        assert_eq!(digests, 2, "the complete snapshot and the 64th delta");
        let (_, rows) = client.view().unwrap();
        assert!(rows[0].cells[0].style.underline, "projected");
        assert!(!rows[0].cells[1].style.underline);
    }

    #[test]
    fn a_test_minor_client_of_a_minor_0_server_digests_at_minor_0() {
        let snapshot = vectors::sample_snapshot();
        let mut server: UpdateEncoder<Arc<TerminalSnapshot>> =
            UpdateEncoder::new(vectors::viewer(), 0);
        let mut client: ViewerView<Arc<TestRow>> = ViewerView::new(0);
        for _ in 0..=UPDATE_DIGEST_INTERVAL {
            let update = minor_0_wire(&server.next(&snapshot, true).unwrap());
            assert_eq!(
                client.apply(&update).unwrap().outcome,
                UpdateOutcome::Applied
            );
        }
        let (_, rows) = client.view().unwrap();
        assert!(rows.iter().all(|row| row.curly.iter().all(|curly| !curly)));
    }

    #[test]
    fn test_minor_peers_keep_the_flag_and_a_minor_0_decoder_refuses_it() {
        let kept = test_row("$ ", [true, false]);
        let mut server: UpdateEncoder<Arc<TestSnapshot>> =
            UpdateEncoder::new(vectors::viewer(), TEST_MINOR);
        let mut client: ViewerView<Arc<TestRow>> = ViewerView::new(TEST_MINOR);
        let mut sent = Vec::new();
        for step in 0..=UPDATE_DIGEST_INTERVAL {
            let current = viewport(step, &kept);
            let update = server.next(&current, true).unwrap();
            let mut w = Writer::new();
            encode_update(&mut w, vectors::scope(), &update, TEST_STYLE_FLAGS)
                .unwrap();
            let bytes = w.into_bytes();
            let received = decode_update(
                &mut Reader::new(&bytes),
                vectors::scope(),
                TEST_STYLE_FLAGS,
            )
            .unwrap();
            assert_eq!(
                client.apply(&received).unwrap().outcome,
                UpdateOutcome::Applied
            );
            let (_, rows) = client.view().unwrap();
            assert_eq!(rows, current.rows.as_slice());
            sent.push(bytes);
        }
        // The native flag is not minor 0's to read, and a sender withholds it.
        assert_eq!(
            decode_update(
                &mut Reader::new(&sent[0]),
                vectors::scope(),
                crate::update::STYLE_FLAGS
            ),
            Err(DecodeError::Invalid("style flags"))
        );
        let native = server.next(&viewport(0, &kept), true).unwrap();
        assert_eq!(
            encode_message(
                &Message::TerminalUpdate(native),
                vectors::scope(),
                MessageCaps::for_minor(0)
            ),
            Err(EncodeError::Unsupported)
        );
    }

    const COLUMNS: usize = 10;

    /// One generated piece of a row: a cell kind and its style choices.
    type Piece = (u8, bool, bool, bool, bool);

    /// A row of [`COLUMNS`] cells from generated pieces: ASCII, written
    /// spaces, cells with no text, U+0000, combining sequences, wide
    /// characters with derivable or mismatched spacers, and runs of eight
    /// identical cells, in two colors, bold or not, and curly underlined or
    /// not.
    fn generated_row(pieces: &[Piece]) -> Arc<TestRow> {
        let mut cells = Vec::new();
        let mut curly = Vec::new();
        let mut pieces = pieces.iter().cycle();
        while cells.len() < COLUMNS {
            let &(kind, red, bold, curled, spacer_curled) =
                pieces.next().unwrap();
            let style = CellStyle {
                bold,
                ..CellStyle::default()
            };
            let foreground = if red {
                CellColor::Indexed(1)
            } else {
                CellColor::DefaultForeground
            };
            let cell = |text: &str, style: CellStyle| Cell {
                text: CellText::from(text),
                foreground,
                background: CellColor::DefaultBackground,
                style,
            };
            let room = COLUMNS - cells.len();
            match kind % 9 {
                0 => cells.push(cell(if bold { "a" } else { "b" }, style)),
                1 => cells.push(cell(" ", style)),
                3 => cells.push(cell("\0", style)),
                4 => cells.push(cell("e\u{301}", style)),
                5 | 6 if room >= 2 => {
                    let wide = CellStyle {
                        wide: true,
                        ..style
                    };
                    let spacer = CellStyle {
                        wide: false,
                        wide_spacer: true,
                        ..style
                    };
                    cells.push(cell(if red { "界" } else { "🙂" }, wide));
                    let mut tail = cell("", spacer);
                    if kind % 9 == 6 {
                        // A spacer in another color is not derivable.
                        tail.foreground = CellColor::Indexed(2);
                    }
                    cells.push(tail);
                    curly.push(curled);
                }
                7 | 8 if room >= 8 => {
                    let text = if kind % 9 == 7 { "" } else { "-" };
                    cells.extend((0..8).map(|_| cell(text, style)));
                    curly.extend([curled; 7]);
                }
                // No text, and a wide character or repeat with no room.
                _ => cells.push(cell("", style)),
            }
            curly.push(if kind % 9 == 5 || kind % 9 == 6 {
                spacer_curled
            } else {
                curled
            });
        }
        curly.truncate(COLUMNS);
        Arc::new(TestRow {
            row: TerminalRow {
                cells,
                ..TerminalRow::default()
            },
            curly,
        })
    }

    fn piece() -> impl proptest::strategy::Strategy<Value = Piece> {
        use proptest::prelude::*;
        (
            0..9_u8,
            any::<bool>(),
            any::<bool>(),
            any::<bool>(),
            any::<bool>(),
        )
    }

    /// The row a minor 0 client shows: curly underlines become underlines.
    fn projected(row: &TestRow) -> Vec<Cell> {
        row.row
            .cells
            .iter()
            .zip(&row.curly)
            .map(|(cell, curly)| {
                let mut cell = cell.clone();
                cell.style.underline |= *curly;
                cell
            })
            .collect()
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(64))]

        /// Clients at minor 0 and at the test-only minor version converge
        /// on rows of every run form, and each view's digest equals the
        /// server's at that minor version after every update.
        #[test]
        fn clients_at_each_minor_converge_on_rows_of_every_run_form(
            steps in proptest::collection::vec(
                (0..3_usize, proptest::collection::vec(piece(), 1..12)),
                1..16,
            ),
        ) {
            let mut current = TestSnapshot {
                metadata: ViewMetadata {
                    size: GridSize { columns: 10, rows: 3 },
                    ..metadata(0)
                },
                rows: vec![generated_row(&[(2, false, false, false, false)]); 3],
            };
            let mut old: UpdateEncoder<Arc<TestSnapshot>> =
                UpdateEncoder::new(vectors::viewer(), 0);
            let mut new: UpdateEncoder<Arc<TestSnapshot>> =
                UpdateEncoder::new(vectors::viewer(), TEST_MINOR);
            let mut old_client: ViewerView = ViewerView::new(0);
            let mut new_client: ViewerView<Arc<TestRow>> =
                ViewerView::new(TEST_MINOR);
            for (revision, (row, pieces)) in (1..).zip(&steps) {
                current.metadata.revision = revision;
                current.rows[*row] = generated_row(pieces);
                let snapshot = Arc::new(current.clone());

                let update = old.next(&snapshot, true).unwrap();
                old_client.apply(&minor_0_wire(&update)).unwrap();
                let (metadata, rows) = old_client.view().unwrap();
                for (shown, sent) in rows.iter().zip(&snapshot.rows) {
                    proptest::prop_assert_eq!(&shown.cells, &projected(sent));
                }
                proptest::prop_assert_eq!(
                    view_digest(metadata, rows, 0),
                    view_digest(&snapshot.metadata, &snapshot.rows, 0)
                );

                let update = new.next(&snapshot, true).unwrap();
                let mut w = Writer::new();
                encode_update(&mut w, vectors::scope(), &update, TEST_STYLE_FLAGS)
                    .unwrap();
                let received = decode_update(
                    &mut Reader::new(&w.into_bytes()),
                    vectors::scope(),
                    TEST_STYLE_FLAGS,
                )
                .unwrap();
                new_client.apply(&received).unwrap();
                let (metadata, rows) = new_client.view().unwrap();
                proptest::prop_assert_eq!(rows, snapshot.rows.as_slice());
                proptest::prop_assert_eq!(
                    view_digest(metadata, rows, TEST_MINOR),
                    view_digest(&snapshot.metadata, &snapshot.rows, TEST_MINOR)
                );
            }
        }
    }
}

#[test]
fn the_test_minor_request_exists_only_at_its_version() {
    use crate::request::{
        Request, RequestId, RequestKind, RequestMessage, RetryClass,
    };
    let message = Message::Request(RequestMessage {
        id: RequestId::FIRST,
        request: Request::PinTab {
            tab: tab_id(4),
            pinned: true,
        },
    });
    assert_eq!(RequestKind::PinTab.since(), TEST_MINOR);
    assert_eq!(RequestKind::PinTab.retry_class(), RetryClass::Mutation);
    assert_eq!(
        encode_message(&message, vectors::scope(), MessageCaps::for_minor(0)),
        Err(EncodeError::Unsupported)
    );
    let bytes = encode_message(
        &message,
        vectors::scope(),
        MessageCaps::for_minor(TEST_MINOR),
    )
    .unwrap();
    // Kind, record length, identity, tag 45, then the tab and the flag.
    assert_eq!(bytes, [0x0e, 0x05, 0x01, 0x2d, 0x02, 0x04, 0x01]);
    assert_eq!(
        decode_message(
            &bytes,
            Direction::ClientToServer,
            vectors::context(TEST_MINOR)
        ),
        Ok(message)
    );
    assert_eq!(
        decode_message(&bytes, Direction::ClientToServer, vectors::context(0)),
        Err(DecodeError::UnknownTag {
            field: "request",
            tag: 45
        })
    );
}
