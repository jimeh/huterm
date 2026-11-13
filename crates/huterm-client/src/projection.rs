//! The client's hierarchy projection, the sequence waiters parked on it,
//! and the structural digest checkpoints results leave for it.
//!
//! Production holds only the minor version 0 projection, a
//! [`HierarchyState`]. Under `test-minor`, a projection built on
//! [`TestHierarchy`] lets a client at the test-only minor version follow a
//! server at that version and compute digests per the negotiated minor
//! version. The seam is the [`ProjectionKind`] and [`Projection`] enums.

use std::collections::BTreeSet;

use huterm_protocol::{
    ApplyOutcome, HierarchyEnvelope, HierarchyEvent, HierarchyState, Touched,
};
use huterm_wire::caps::Direction;
use huterm_wire::codec::DecodeError;
use huterm_wire::digest_profile;
use huterm_wire::message::{DecodeContext, Message, decode_message};
#[cfg(any(test, feature = "test-minor"))]
use huterm_wire::test_minor::{TEST_MINOR, TestEvent, TestHierarchy};

use crate::session::{MAX_PENDING_CHECKPOINTS, WaitId};

/// Which projection a session holds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProjectionKind {
    /// The minor version 0 projection.
    Base,
    /// The test-only minor version's projection.
    #[cfg(any(test, feature = "test-minor"))]
    Test,
}

/// The client's copy of the server's hierarchy.
#[derive(Clone, Debug)]
pub(crate) enum Projection {
    Base(HierarchyState),
    #[cfg(any(test, feature = "test-minor"))]
    Test(TestHierarchy),
}

/// A decoded hierarchy event.
#[derive(Debug)]
pub(crate) enum Incoming {
    Base(HierarchyEvent),
    #[cfg(any(test, feature = "test-minor"))]
    Test(TestEvent),
}

/// A decoded hierarchy event with its sequence and digest.
#[derive(Debug)]
pub(crate) struct Sequenced {
    pub(crate) seq: u64,
    pub(crate) event: Incoming,
    pub(crate) digest: Option<u64>,
}

/// What applying one event did.
#[derive(Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "an outcome lives only while its event applies, as core's does"
)]
pub(crate) enum Outcome {
    /// It applied; the summary names what changed.
    Applied(Touched),
    /// The projection already holds its sequence.
    Stale,
    /// A gap or an inconsistent event: the projection needs a snapshot.
    Resync,
}

impl ProjectionKind {
    /// Decodes a `HierarchySnapshot` or `HierarchyResync` message into its
    /// digest and projection.
    pub(crate) fn decode_snapshot(
        self,
        bytes: &[u8],
        context: DecodeContext,
    ) -> Result<(u64, Projection), DecodeError> {
        match self {
            Self::Base => {
                match decode_message(bytes, Direction::ServerToClient, context)?
                {
                    Message::HierarchySnapshot(snapshot)
                    | Message::HierarchyResync(snapshot) => {
                        Ok((snapshot.digest, Projection::Base(snapshot.state)))
                    }
                    _ => Err(DecodeError::Invalid("hierarchy snapshot")),
                }
            }
            #[cfg(any(test, feature = "test-minor"))]
            Self::Test => {
                let mut reader = huterm_wire::codec::Reader::new(bytes);
                reader.u64()?;
                let (digest, hierarchy) =
                    huterm_wire::test_minor::decode_snapshot(
                        &mut reader,
                        context.hierarchy,
                    )?;
                reader.finish()?;
                Ok((digest, Projection::Test(hierarchy)))
            }
        }
    }

    /// Decodes a `HierarchyEvent` message.
    pub(crate) fn decode_event(
        self,
        bytes: &[u8],
        context: DecodeContext,
    ) -> Result<Sequenced, DecodeError> {
        match self {
            Self::Base => {
                match decode_message(bytes, Direction::ServerToClient, context)?
                {
                    Message::HierarchyEvent(event) => Ok(Sequenced {
                        seq: event.seq,
                        event: Incoming::Base(event.event),
                        digest: event.digest,
                    }),
                    _ => Err(DecodeError::Invalid("hierarchy event")),
                }
            }
            #[cfg(any(test, feature = "test-minor"))]
            Self::Test => decode_test_event(bytes, context),
        }
    }
}

/// Decodes an event at the test-only minor version, which holds minor
/// version 0's events too; a newer event at minor version 0 is invalid.
#[cfg(any(test, feature = "test-minor"))]
fn decode_test_event(
    bytes: &[u8],
    context: DecodeContext,
) -> Result<Sequenced, DecodeError> {
    let mut reader = huterm_wire::codec::Reader::new(bytes);
    reader.u64()?;
    let scope = context.hierarchy.scope();
    let (seq, event, digest) = reader.record(|r| {
        Ok((
            r.u64()?,
            huterm_wire::test_minor::decode_test_event(r, scope)?,
            r.option("event digest", |r| Ok(u64::from_le_bytes(r.fixed()?)))?,
        ))
    })?;
    reader.finish()?;
    if context.caps.minor() < TEST_MINOR && !matches!(event, TestEvent::Base(_))
    {
        return Err(DecodeError::Invalid("hierarchy event above the minor"));
    }
    Ok(Sequenced {
        seq,
        event: Incoming::Test(event),
        digest,
    })
}

impl Projection {
    /// The minor version 0 state.
    pub(crate) fn state(&self) -> &HierarchyState {
        match self {
            Self::Base(state) => state,
            #[cfg(any(test, feature = "test-minor"))]
            Self::Test(hierarchy) => &hierarchy.state,
        }
    }

    /// The sequence the projection holds.
    pub(crate) fn seq(&self) -> u64 {
        self.state().seq()
    }

    /// The digest at negotiated minor version `minor`.
    pub(crate) fn digest(&self, minor: u16) -> u64 {
        match self {
            Self::Base(state) => state.digest_for(digest_profile(minor)),
            #[cfg(any(test, feature = "test-minor"))]
            Self::Test(hierarchy) => hierarchy.digest(minor),
        }
    }

    /// Applies the event at `seq`.
    pub(crate) fn apply(&mut self, seq: u64, event: Incoming) -> Outcome {
        match self {
            Self::Base(state) => {
                let envelope = HierarchyEnvelope {
                    stream: state.stream(),
                    seq,
                    event: match event {
                        Incoming::Base(event) => event,
                        // A minor version 0 projection never decodes one.
                        #[cfg(any(test, feature = "test-minor"))]
                        Incoming::Test(_) => return Outcome::Resync,
                    },
                };
                match state.apply(envelope) {
                    ApplyOutcome::Applied(touched) => Outcome::Applied(touched),
                    ApplyOutcome::Stale => Outcome::Stale,
                    ApplyOutcome::ResyncRequired(_) => Outcome::Resync,
                }
            }
            #[cfg(any(test, feature = "test-minor"))]
            Self::Test(hierarchy) => {
                let current = hierarchy.state.seq();
                if seq <= current {
                    return Outcome::Stale;
                }
                if current.checked_add(1) != Some(seq) {
                    return Outcome::Resync;
                }
                let event = match event {
                    Incoming::Base(event) => TestEvent::Base(event),
                    Incoming::Test(event) => event,
                };
                match hierarchy.apply(&event, TEST_MINOR) {
                    Some(_) => Outcome::Applied(Touched::everything()),
                    None => Outcome::Resync,
                }
            }
        }
    }
}

/// One-shot sequence waiters, released in sequence order. Each resolves
/// exactly once: release and cancellation remove it before reporting it.
#[derive(Debug, Default)]
pub(crate) struct Waiters {
    waiting: BTreeSet<(u64, WaitId)>,
}

impl Waiters {
    pub(crate) fn insert(&mut self, seq: u64, wait: WaitId) {
        self.waiting.insert((seq, wait));
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.waiting.is_empty()
    }

    /// Removes and reports every waiter whose sequence is at most
    /// `applied`.
    pub(crate) fn release(
        &mut self,
        applied: u64,
        mut ready: impl FnMut(WaitId),
    ) {
        while let Some(&(seq, wait)) = self.waiting.first() {
            if seq > applied {
                break;
            }
            self.waiting.pop_first();
            ready(wait);
        }
    }

    /// Removes and reports every waiter.
    pub(crate) fn cancel_all(&mut self, mut cancelled: impl FnMut(WaitId)) {
        while let Some((_, wait)) = self.waiting.pop_first() {
            cancelled(wait);
        }
    }
}

/// Structural digests results reported for sequences the projection has
/// not reached yet, at most [`MAX_PENDING_CHECKPOINTS`].
#[derive(Debug, Default)]
pub(crate) struct Checkpoints {
    pending: BTreeSet<(u64, u64)>,
}

impl Checkpoints {
    /// Keeps a checkpoint, dropping the oldest past the bound; returns
    /// the sequence of the one it dropped.
    pub(crate) fn insert(&mut self, seq: u64, digest: u64) -> Option<u64> {
        self.pending.insert((seq, digest));
        if self.pending.len() > MAX_PENDING_CHECKPOINTS {
            self.pending.pop_first().map(|(dropped, _)| dropped)
        } else {
            None
        }
    }

    /// Takes the digests to check now that the projection applied
    /// `applied`, dropping those for sequences it passed.
    pub(crate) fn reached(&mut self, applied: u64, mut check: impl FnMut(u64)) {
        while let Some(&(seq, digest)) = self.pending.first() {
            if seq > applied {
                break;
            }
            self.pending.pop_first();
            if seq == applied {
                check(digest);
            }
        }
    }

    /// Drops every checkpoint a snapshot at `seq` covers, unchecked: the
    /// snapshot carries its own digest.
    pub(crate) fn covered(&mut self, seq: u64) {
        while self
            .pending
            .first()
            .is_some_and(|(pending, _)| *pending <= seq)
        {
            self.pending.pop_first();
        }
    }

    pub(crate) fn clear(&mut self) {
        self.pending.clear();
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.pending.len()
    }
}
