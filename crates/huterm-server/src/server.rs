//! State every connection of one server shares.

use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use huterm_core::Mux;
use huterm_protocol::RuntimeId;
use huterm_wire::handshake::ServerOffer;

use crate::config::ServerConfig;
use crate::connection::Connection;
use crate::feed::{CoreSource, HierarchySource};

/// A server: the runtime it serves, what it offers each client, its
/// limits, and counts of what its connections hold.
pub struct Server {
    mux: Arc<Mutex<Mux>>,
    runtime: RuntimeId,
    offer: ServerOffer,
    config: ServerConfig,
    hierarchy: Arc<dyn HierarchySource>,
    counters: Arc<Counters>,
}

impl fmt::Debug for Server {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Server")
            .field("runtime", &self.runtime)
            .field("offer", &self.offer)
            .field("config", &self.config)
            .field("counters", &self.counters)
            .finish_non_exhaustive()
    }
}

impl Server {
    /// A server for `mux` that offers `offer` and applies `config`.
    pub fn new(
        mux: Arc<Mutex<Mux>>,
        offer: ServerOffer,
        config: ServerConfig,
    ) -> Self {
        let runtime = lock(&mux).runtime_id();
        Self {
            mux,
            runtime,
            offer,
            config,
            hierarchy: Arc::new(CoreSource),
            counters: Arc::default(),
        }
    }

    /// Replaces where hierarchy subscriptions come from, such as the test
    /// feed. Production servers keep core's digesting subscription.
    #[must_use]
    pub fn with_hierarchy_source(
        mut self,
        source: Arc<dyn HierarchySource>,
    ) -> Self {
        self.hierarchy = source;
        self
    }

    /// The runtime this server serves.
    #[must_use]
    pub fn mux(&self) -> &Arc<Mutex<Mux>> {
        &self.mux
    }

    /// Locks the runtime, taking it back from a poisoned lock as the
    /// desktop does.
    pub fn lock(&self) -> MutexGuard<'_, Mux> {
        lock(&self.mux)
    }

    /// The runtime scope every identity on this server's connections
    /// travels in.
    #[must_use]
    pub fn runtime(&self) -> RuntimeId {
        self.runtime
    }

    /// What this server offers each client.
    #[must_use]
    pub fn offer(&self) -> &ServerOffer {
        &self.offer
    }

    /// The limits this server applies.
    #[must_use]
    pub fn config(&self) -> ServerConfig {
        self.config
    }

    /// Where hierarchy subscriptions come from.
    #[must_use]
    pub fn hierarchy(&self) -> &Arc<dyn HierarchySource> {
        &self.hierarchy
    }

    /// A new connection accepted at `now` on the connection's clock.
    #[must_use]
    pub fn connection(&self, now: Duration) -> Connection {
        Connection::new(
            self.offer.clone(),
            self.config,
            self.runtime,
            Arc::clone(&self.counters),
            now,
        )
    }

    /// Close tickets held by every connection, including those whose
    /// record a running job holds.
    #[must_use]
    pub fn live_close_tickets(&self) -> usize {
        self.counters.tickets.load(Ordering::Acquire)
    }

    /// Layout history records held by every connection, including those a
    /// running restore holds.
    #[must_use]
    pub fn live_history_records(&self) -> usize {
        self.counters.records.load(Ordering::Acquire)
    }
}

fn lock(mux: &Mutex<Mux>) -> MutexGuard<'_, Mux> {
    mux.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Server-wide counts of per-connection records, so tests can observe that
/// a disconnect released them.
#[derive(Debug, Default)]
pub(crate) struct Counters {
    tickets: AtomicUsize,
    records: AtomicUsize,
}

/// Which count a [`Live`] record adds to.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Record {
    Ticket,
    History,
}

/// Counts one live record until it is dropped, wherever it travels: in a
/// connection's table, or in a job a worker runs.
#[derive(Debug)]
pub(crate) struct Live {
    counters: Arc<Counters>,
    record: Record,
}

impl Live {
    pub(crate) fn new(counters: &Arc<Counters>, record: Record) -> Self {
        Self::counter(counters, record).fetch_add(1, Ordering::AcqRel);
        Self {
            counters: Arc::clone(counters),
            record,
        }
    }

    fn counter(counters: &Counters, record: Record) -> &AtomicUsize {
        match record {
            Record::Ticket => &counters.tickets,
            Record::History => &counters.records,
        }
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        Self::counter(&self.counters, self.record)
            .fetch_sub(1, Ordering::AcqRel);
    }
}
