//! The hierarchy lane's feed and its replacements.
//!
//! Subscribe, a client's resync, and a lag resync all replace the feed:
//! work that captures a snapshot and a new feed under one lock. At most one
//! replacement is in flight; requests that arrive meanwhile wait for it,
//! and a lag noticed meanwhile needs nothing more. Once a replacement is
//! issued the connection stops draining the old feed, and on completion it
//! drops the old feed, queues the snapshot, and drains the new one, whose
//! first event follows the snapshot. Each feed has a generation, and a
//! completion for any replacement but the one in flight is dropped, so the
//! lane never carries an older snapshot after newer events.

use std::sync::Arc;

use huterm_wire::request::RequestId;

use crate::feed::{HierarchyFeed, SnapshotKind};
use crate::work::WorkId;

#[derive(Debug)]
struct InFlight {
    work: WorkId,
    generation: u64,
    waiters: Vec<RequestId>,
}

#[derive(Debug, Default)]
pub(super) struct HierarchyLane {
    feed: Option<(u64, Arc<dyn HierarchyFeed>)>,
    /// Whether the client asked to subscribe, so structural results carry
    /// digests.
    requested: bool,
    /// Whether a snapshot was queued, so later replacements are resyncs.
    subscribed: bool,
    lagged: bool,
    /// A replacement is wanted and not yet issued, with the requests it
    /// answers.
    pending: Option<Vec<RequestId>>,
    in_flight: Option<InFlight>,
    last_generation: u64,
}

impl HierarchyLane {
    pub(super) fn requested(&self) -> bool {
        self.requested
    }

    /// The feed being drained, with its generation.
    pub(super) fn feed(&self) -> Option<(u64, Arc<dyn HierarchyFeed>)> {
        self.feed
            .as_ref()
            .map(|(generation, feed)| (*generation, Arc::clone(feed)))
    }

    /// The feed to drain now: none while a replacement is wanted or runs.
    pub(super) fn draining(&self) -> Option<&Arc<dyn HierarchyFeed>> {
        if self.lagged || self.pending.is_some() || self.in_flight.is_some() {
            return None;
        }
        self.feed.as_ref().map(|(_, feed)| feed)
    }

    /// A subscribe or resync request that the next replacement answers.
    pub(super) fn want(&mut self, request: RequestId) {
        self.requested = true;
        match &mut self.in_flight {
            Some(in_flight) => in_flight.waiters.push(request),
            None => self.pending.get_or_insert_with(Vec::new).push(request),
        }
    }

    /// The feed lagged: replace it once the lane drains.
    pub(super) fn lag(&mut self) {
        self.lagged = true;
        if self.in_flight.is_none() {
            self.pending.get_or_insert_with(Vec::new);
        }
    }

    /// The feed ended.
    pub(super) fn closed(&mut self) {
        self.feed = None;
    }

    /// Withdraws a request that waits for a replacement not yet issued;
    /// false when it is not waiting there.
    pub(super) fn cancel(&mut self, request: RequestId) -> bool {
        let Some(waiters) = &mut self.pending else {
            return false;
        };
        let Some(index) = waiters.iter().position(|id| *id == request) else {
            return false;
        };
        waiters.remove(index);
        if waiters.is_empty() && !self.lagged {
            self.pending = None;
        }
        true
    }

    /// Whether a replacement waits to be issued.
    pub(super) fn wants_replacement(&self) -> bool {
        self.pending.is_some() && self.in_flight.is_none()
    }

    /// Issues the wanted replacement as `work`.
    pub(super) fn issue(&mut self, work: WorkId) -> SnapshotKind {
        self.last_generation += 1;
        self.in_flight = Some(InFlight {
            work,
            generation: self.last_generation,
            waiters: self.pending.take().unwrap_or_default(),
        });
        if self.subscribed {
            SnapshotKind::Resync
        } else {
            SnapshotKind::Snapshot
        }
    }

    /// Ends the replacement `work`, returning its generation and the
    /// requests it answers, or `None` for a replacement no longer in
    /// flight.
    pub(super) fn complete(
        &mut self,
        work: WorkId,
    ) -> Option<(u64, Vec<RequestId>)> {
        if self.in_flight.as_ref().is_none_or(|f| f.work != work) {
            return None;
        }
        let in_flight = self.in_flight.take()?;
        Some((in_flight.generation, in_flight.waiters))
    }

    /// Installs a replacement's feed, dropping the old one.
    pub(super) fn install(
        &mut self,
        generation: u64,
        feed: Box<dyn HierarchyFeed>,
    ) {
        self.feed = Some((generation, Arc::from(feed)));
        self.subscribed = true;
        self.lagged = false;
    }

    /// A replacement failed; a lag still wants one.
    pub(super) fn failed(&mut self) {
        if self.lagged {
            self.pending.get_or_insert_with(Vec::new);
        }
    }

    /// Drops the feed and any wanted replacement.
    pub(super) fn clear(&mut self) {
        self.feed = None;
        self.pending = None;
        self.lagged = false;
    }
}
