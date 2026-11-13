//! A hierarchy source at the test-only minor version.
//!
//! [`TestSource`] holds a [`TestHierarchy`] instead of reading the `Mux`.
//! A test emits [`TestEvent`]s through it, and each connection's feed
//! delivers every event at its negotiated minor version: the event itself
//! to a peer at [`TEST_MINOR`], and to a minor version 0 peer the
//! `Skipped`, the one older event, or the projected `HierarchyResync` that
//! [`TestHierarchy::apply`] chooses, at the event's sequence and with the
//! peer's digests. Structural requests still run against the `Mux`, so a
//! test that uses this source drives the hierarchy only through it.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError, Weak};

use huterm_core::Mux;
use huterm_protocol::{HIERARCHY_DIGEST_INTERVAL, HierarchyState};
use huterm_wire::caps::MessageKind;
use huterm_wire::codec::{EncodeError, Writer};
use huterm_wire::hierarchy::{HierarchySnapshot, SequencedEvent};
use huterm_wire::message::{Message, encode_message};
use huterm_wire::test_minor::{
    Delivery, TEST_MINOR, TestEvent, TestHierarchy, encode_snapshot,
    encode_test_event,
};

use crate::feed::{
    Activity, EncodeSnapshot, FeedEncoding, FeedError, HierarchyFeed,
    HierarchySource, SnapshotKind, Subscribed,
};

/// A hierarchy source driven by test events.
#[derive(Clone, Debug)]
pub struct TestSource {
    shared: Arc<Mutex<Shared>>,
}

#[derive(Debug)]
struct Shared {
    hierarchy: TestHierarchy,
    feeds: Vec<Weak<Inner>>,
}

#[derive(Debug)]
enum Item {
    Native {
        seq: u64,
        event: TestEvent,
        digest: Option<u64>,
    },
    Event(SequencedEvent),
    Resync(HierarchySnapshot),
}

#[derive(Debug)]
struct Inner {
    minor: u16,
    encoding: FeedEncoding,
    queue: Mutex<VecDeque<Item>>,
    wake: async_channel::Sender<()>,
    signal: async_channel::Receiver<()>,
}

impl TestSource {
    /// A source whose hierarchy starts as `state`, with no pinned tabs.
    #[must_use]
    pub fn new(state: HierarchyState) -> Self {
        Self {
            shared: Arc::new(Mutex::new(Shared {
                hierarchy: TestHierarchy::new(state),
                feeds: Vec::new(),
            })),
        }
    }

    /// The hierarchy as it stands.
    #[must_use]
    pub fn hierarchy(&self) -> TestHierarchy {
        self.lock().hierarchy.clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Applies `event` and delivers it to every live feed, returning its
    /// sequence, or `None` when it does not apply.
    pub fn emit(&self, event: &TestEvent) -> Option<u64> {
        let mut shared = self.lock();
        let older = shared.hierarchy.apply(event, 0)?;
        let hierarchy = &shared.hierarchy;
        let seq = hierarchy.state.seq();
        let digest = |minor| {
            seq.is_multiple_of(HIERARCHY_DIGEST_INTERVAL)
                .then(|| hierarchy.digest(minor))
        };
        let items: Vec<_> = shared
            .feeds
            .iter()
            .filter_map(Weak::upgrade)
            .map(|feed| {
                let item = if feed.minor >= TEST_MINOR {
                    Item::Native {
                        seq,
                        event: event.clone(),
                        digest: digest(feed.minor),
                    }
                } else {
                    match &older {
                        Delivery::Event(event) => Item::Event(SequencedEvent {
                            seq,
                            event: event.clone(),
                            digest: digest(feed.minor),
                        }),
                        Delivery::Resync(state) => {
                            Item::Resync(HierarchySnapshot {
                                digest: hierarchy.digest(feed.minor),
                                state: state.clone(),
                            })
                        }
                        // `apply` delivers the event itself only to a peer
                        // at the test-only minor version.
                        Delivery::Native(_) => {
                            Item::Resync(HierarchySnapshot {
                                digest: hierarchy.digest(feed.minor),
                                state: hierarchy.project(),
                            })
                        }
                    }
                };
                (feed, item)
            })
            .collect();
        shared.feeds.retain(|feed| feed.strong_count() > 0);
        drop(shared);
        for (feed, item) in items {
            feed.queue
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push_back(item);
            let _ = feed.wake.try_send(());
        }
        Some(seq)
    }
}

impl HierarchySource for TestSource {
    fn subscribe(&self, _mux: &mut Mux, encoding: FeedEncoding) -> Subscribed {
        let minor = encoding.caps.minor();
        let (wake, signal) = async_channel::bounded(1);
        let inner = Arc::new(Inner {
            minor,
            encoding,
            queue: Mutex::new(VecDeque::new()),
            wake,
            signal,
        });
        let mut shared = self.lock();
        shared.feeds.push(Arc::downgrade(&inner));
        let hierarchy = shared.hierarchy.clone();
        Subscribed {
            seq: hierarchy.state.seq(),
            digest: hierarchy.digest(minor),
            snapshot: Box::new(TestSnapshot { hierarchy, minor }),
            feed: Box::new(TestFeed(inner)),
        }
    }
}

#[derive(Debug)]
struct TestSnapshot {
    hierarchy: TestHierarchy,
    minor: u16,
}

impl EncodeSnapshot for TestSnapshot {
    fn encode(
        self: Box<Self>,
        kind: SnapshotKind,
        encoding: FeedEncoding,
    ) -> Result<Vec<u8>, EncodeError> {
        let message_kind = match kind {
            SnapshotKind::Snapshot => MessageKind::HierarchySnapshot,
            SnapshotKind::Resync => MessageKind::HierarchyResync,
        };
        if self.minor < TEST_MINOR {
            let snapshot = HierarchySnapshot {
                digest: self.hierarchy.digest(self.minor),
                state: self.hierarchy.project(),
            };
            let message = match kind {
                SnapshotKind::Snapshot => Message::HierarchySnapshot(snapshot),
                SnapshotKind::Resync => Message::HierarchyResync(snapshot),
            };
            return encode_message(&message, encoding.scope, encoding.caps);
        }
        let mut w = Writer::new();
        w.u64(message_kind.number());
        encode_snapshot(&mut w, encoding.scope, &self.hierarchy)?;
        Ok(w.into_bytes())
    }
}

#[derive(Debug)]
struct TestFeed(Arc<Inner>);

impl HierarchyFeed for TestFeed {
    fn next(&self) -> Result<Vec<u8>, FeedError> {
        let item = self
            .0
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front()
            .ok_or(FeedError::Empty)?;
        let FeedEncoding { scope, caps } = self.0.encoding;
        let encoded = match item {
            Item::Native { seq, event, digest } => {
                let mut w = Writer::new();
                w.u64(MessageKind::HierarchyEvent.number());
                w.record(|w| {
                    w.u64(seq);
                    encode_test_event(w, scope, &event)?;
                    w.option(digest, |w, digest| {
                        w.fixed(&digest.to_le_bytes());
                        Ok(())
                    })
                })
                .map(|()| w.into_bytes())
            }
            Item::Event(event) => {
                encode_message(&Message::HierarchyEvent(event), scope, caps)
            }
            Item::Resync(snapshot) => {
                encode_message(&Message::HierarchyResync(snapshot), scope, caps)
            }
        };
        encoded.map_err(FeedError::Encode)
    }

    fn activity(&self) -> Activity<'_> {
        Box::pin(async move { self.0.signal.recv().await.is_ok() })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use huterm_protocol::{ApplyOutcome, HierarchyEnvelope, HierarchyEvent};
    use huterm_wire::codec::Reader;
    use huterm_wire::handshake::{ClientKind, Hello};
    use huterm_wire::request::Request;
    use huterm_wire::test_minor::{decode_snapshot, decode_test_event};

    use super::*;
    use crate::config::ServerConfig;
    use crate::connection::Connection;
    use crate::server::Server;
    use crate::test_support::{TestPeer, flush, offer, quiet_command, run};

    const T0: Duration = Duration::ZERO;

    /// A subscribed connection at `minor` and its peer.
    fn subscribed(
        server: &Server,
        minor: u16,
    ) -> (Connection, TestPeer, Vec<Vec<u8>>) {
        let mut connection = server.connection(T0);
        let mut peer = TestPeer::with_hello(
            server.runtime(),
            Hello {
                minor,
                ..Hello::new(ClientKind::Desktop, "test", "test")
            },
        );
        connection.receive(&peer.opening(), T0);
        let (_, subscribe) = peer.request(Request::SubscribeHierarchy);
        connection.receive(&subscribe, T0);
        run(server, &mut connection, T0);
        let messages = peer.receive_raw(&flush(&mut connection, T0));
        (connection, peer, messages)
    }

    fn kind(message: &[u8]) -> u64 {
        Reader::new(message).u64().unwrap()
    }

    /// The snapshot among a subscription's first messages.
    fn snapshot_message(messages: &[Vec<u8>]) -> &[u8] {
        messages
            .iter()
            .find(|message| {
                kind(message) == MessageKind::HierarchySnapshot.number()
            })
            .expect("a snapshot")
    }

    /// Applies what a minor version 0 peer received to its projection.
    fn follow(projection: &mut HierarchyState, delivered: &[Message]) {
        for message in delivered {
            match message {
                Message::HierarchyEvent(event) => {
                    let outcome = projection.apply(HierarchyEnvelope {
                        stream: projection.stream(),
                        seq: event.seq,
                        event: event.event.clone(),
                    });
                    assert!(matches!(outcome, ApplyOutcome::Applied(_)));
                }
                Message::HierarchyResync(resync) => {
                    assert_eq!(resync.state.digest(), resync.digest);
                    *projection = resync.state.clone();
                }
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn each_test_event_reaches_a_minor_0_peer_downgraded_and_a_test_minor_peer_as_itself()
     {
        let mux = Arc::new(Mutex::new(Mux::default()));
        let (first, second, state) = {
            let mut mux = mux.lock().unwrap();
            let session = mux.create_session(None).unwrap();
            let workspace = mux.create_workspace(session, None).unwrap();
            let mut open =
                || mux.open_tab(workspace, &quiet_command()).unwrap().tab.id;
            let (first, second) = (open(), open());
            (first, second, mux.subscribe_hierarchy().0)
        };
        let source = TestSource::new(state);
        let server = Server::new(
            Arc::clone(&mux),
            offer(TEST_MINOR),
            ServerConfig::default(),
        )
        .with_hierarchy_source(Arc::new(source.clone()));
        let (mut old, mut old_peer, opening) = subscribed(&server, 0);
        let snapshot = snapshot_message(&opening);
        let Message::HierarchySnapshot(snapshot) = old_peer.decode(snapshot)
        else {
            unreachable!();
        };
        let mut projection = snapshot.state;
        let (mut new, mut new_peer, opening) = subscribed(&server, TEST_MINOR);
        let snapshot = snapshot_message(&opening);
        let mut reader = Reader::new(snapshot);
        reader.u64().unwrap();
        let context = huterm_wire::hierarchy::HierarchyContext {
            stream: new_peer.stream(),
        };
        let (digest, hierarchy) =
            decode_snapshot(&mut reader, context).unwrap();
        assert_eq!(digest, source.hierarchy().digest(TEST_MINOR));
        assert_eq!(hierarchy.state.seq(), projection.seq());

        let start = projection.seq();
        let events = [
            TestEvent::TabPinned {
                tab: first,
                pinned: true,
            },
            TestEvent::TabRetitled {
                tab: second,
                custom_name: Some("retitled".into()),
            },
            TestEvent::TabsExchanged { first, second },
        ];
        for (offset, event) in (1..).zip(&events) {
            assert_eq!(source.emit(event), Some(start + offset));
        }

        old.pump(T0);
        let delivered: Vec<Message> = old_peer.receive(&flush(&mut old, T0));
        assert!(
            matches!(
                delivered.as_slice(),
                [
                    Message::HierarchyEvent(skipped),
                    Message::HierarchyEvent(renamed),
                    Message::HierarchyResync(resync),
                ] if skipped.seq == start + 1
                    && skipped.event == HierarchyEvent::Skipped
                    && renamed.seq == start + 2
                    && matches!(renamed.event, HierarchyEvent::TabRenamed { tab, .. } if tab == second)
                    && resync.state.seq() == start + 3
            ),
            "{delivered:?}"
        );
        follow(&mut projection, &delivered);
        let current = source.hierarchy();
        assert_eq!(projection.digest(), current.digest(0));
        assert_eq!(
            projection.workspace_tabs(projection.tab_workspace(first).unwrap()),
            Some(&[second, first][..])
        );

        new.pump(T0);
        let native: Vec<TestEvent> = new_peer
            .receive_raw(&flush(&mut new, T0))
            .iter()
            .map(|message| {
                let mut reader = Reader::new(message);
                assert_eq!(
                    reader.u64().unwrap(),
                    MessageKind::HierarchyEvent.number()
                );
                reader
                    .record(|r| {
                        r.u64()?;
                        decode_test_event(r, context.scope())
                    })
                    .unwrap()
            })
            .collect();
        assert_eq!(native, events);
        assert!(current.pinned.contains(&first));
        let _ = mux.lock().unwrap().shutdown();
    }
}
