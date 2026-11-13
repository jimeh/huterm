//! Where a connection's hierarchy snapshot and events come from.
//!
//! A [`HierarchySource`] captures a snapshot and a new feed of later events
//! under one Mux lock. Production uses [`CoreSource`], core's digesting
//! subscription. Tests replace the source, as the test feed does to deliver
//! test-only minor version events, without a test branch here.
//!
//! A feed hands out each event already encoded for its connection, so the
//! connection never re-encodes and a source with its own event kinds
//! encodes them itself.

use std::fmt;
use std::future::Future;
use std::pin::Pin;

use huterm_core::{DigestedHierarchySubscription, HierarchyRecvError, Mux};
use huterm_wire::caps::MessageCaps;
use huterm_wire::codec::{EncodeError, IdScope};
use huterm_wire::digest_profile;
use huterm_wire::hierarchy::{HierarchySnapshot, SequencedEvent};
use huterm_wire::message::{Message, encode_message};

/// How a connection encodes hierarchy messages: its identity scope and
/// negotiated caps, whose minor version picks the digest profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FeedEncoding {
    /// The connection's identity scope.
    pub scope: IdScope,
    /// The negotiated caps.
    pub caps: MessageCaps,
}

/// Which message carries a snapshot: the first on a connection, or one
/// that replaces events the client did not get.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapshotKind {
    /// `HierarchySnapshot`, the first a connection receives.
    Snapshot,
    /// `HierarchyResync`, every later replacement.
    Resync,
}

/// Why a feed handed out no event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FeedError {
    /// No event is queued.
    Empty,
    /// Events were discarded; the connection must replace the feed.
    Lagged,
    /// The feed ended and nothing remains.
    Closed,
    /// An event could not be encoded, which breaks a core invariant.
    Encode(EncodeError),
}

/// A future that completes once a feed has activity, with `false` once the
/// feed has closed.
pub type Activity<'a> = Pin<Box<dyn Future<Output = bool> + Send + 'a>>;

/// A connection's feed of hierarchy events after its snapshot.
pub trait HierarchyFeed: Send + Sync + fmt::Debug {
    /// Takes the next event, encoded as a whole message.
    ///
    /// # Errors
    /// Returns why no event was handed out.
    fn next(&self) -> Result<Vec<u8>, FeedError>;

    /// Completes once events, a lag, or the end arrived since the last
    /// call. The signal coalesces: drain with [`Self::next`] until it
    /// reports `Empty`.
    fn activity(&self) -> Activity<'_>;
}

/// A snapshot a source captured, encoded later off the Mux lock.
pub trait EncodeSnapshot: Send + fmt::Debug {
    /// Encodes the snapshot as a whole message of `kind`.
    ///
    /// # Errors
    /// Returns the encoder's error.
    fn encode(
        self: Box<Self>,
        kind: SnapshotKind,
        encoding: FeedEncoding,
    ) -> Result<Vec<u8>, EncodeError>;
}

/// What one subscription captured under the Mux lock.
#[derive(Debug)]
pub struct Subscribed {
    /// The sequence the snapshot is current through.
    pub seq: u64,
    /// The snapshot's digest under the connection's profile.
    pub digest: u64,
    /// The snapshot, not yet encoded.
    pub snapshot: Box<dyn EncodeSnapshot>,
    /// The events after `seq`, from `seq + 1`.
    pub feed: Box<dyn HierarchyFeed>,
}

/// Where hierarchy subscriptions come from.
pub trait HierarchySource: Send + Sync + fmt::Debug {
    /// Captures a snapshot and subscribes to every later event, under the
    /// lock the caller holds on `mux`.
    fn subscribe(&self, mux: &mut Mux, encoding: FeedEncoding) -> Subscribed;
}

/// Core's digesting hierarchy subscription.
#[derive(Clone, Copy, Debug, Default)]
pub struct CoreSource;

impl HierarchySource for CoreSource {
    fn subscribe(&self, mux: &mut Mux, encoding: FeedEncoding) -> Subscribed {
        let profile = digest_profile(encoding.caps.minor());
        let (state, digest, subscription) =
            mux.subscribe_hierarchy_with_digests(profile);
        Subscribed {
            seq: state.seq(),
            digest,
            snapshot: Box::new(CoreSnapshot(HierarchySnapshot {
                digest,
                state,
            })),
            feed: Box::new(CoreFeed {
                subscription,
                encoding,
            }),
        }
    }
}

#[derive(Debug)]
struct CoreSnapshot(HierarchySnapshot);

impl EncodeSnapshot for CoreSnapshot {
    fn encode(
        self: Box<Self>,
        kind: SnapshotKind,
        encoding: FeedEncoding,
    ) -> Result<Vec<u8>, EncodeError> {
        let message = match kind {
            SnapshotKind::Snapshot => Message::HierarchySnapshot(self.0),
            SnapshotKind::Resync => Message::HierarchyResync(self.0),
        };
        encode_message(&message, encoding.scope, encoding.caps)
    }
}

#[derive(Debug)]
struct CoreFeed {
    subscription: DigestedHierarchySubscription,
    encoding: FeedEncoding,
}

impl HierarchyFeed for CoreFeed {
    fn next(&self) -> Result<Vec<u8>, FeedError> {
        let queued =
            self.subscription.try_recv().map_err(|error| match error {
                HierarchyRecvError::Empty => FeedError::Empty,
                HierarchyRecvError::Lagged => FeedError::Lagged,
                HierarchyRecvError::Closed => FeedError::Closed,
            })?;
        let message = Message::HierarchyEvent(SequencedEvent {
            seq: queued.envelope.seq,
            event: queued.envelope.event,
            digest: queued.digest,
        });
        encode_message(&message, self.encoding.scope, self.encoding.caps)
            .map_err(FeedError::Encode)
    }

    fn activity(&self) -> Activity<'_> {
        Box::pin(
            async move { self.subscription.wait_for_activity().await.is_ok() },
        )
    }
}
