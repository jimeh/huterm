//! The server's per-connection limits, and the configuration that lets tests
//! lower them.
//!
//! The wire crate names the limits a client must know, such as
//! [`MAX_IN_FLIGHT_REQUESTS`] and [`WRITER_STALL_DEADLINE`]; the limits
//! here bind only the server's own memory and are never advertised.

use huterm_wire::caps::{MessageCaps, MessageKind};
use huterm_wire::handshake::MAX_IN_FLIGHT_REQUESTS;
#[cfg(doc)]
use huterm_wire::handshake::WRITER_STALL_DEADLINE;

/// The most close tickets one connection holds, a ticket whose record a
/// running job holds included. Past it, `PrepareClose` answers
/// `LimitExceeded`.
pub const MAX_CLOSE_TICKETS: usize = 16;

/// The most layout history slots one connection keeps. Using one more
/// evicts the least recently used slot, whose tokens then answer
/// `StaleUndo`.
pub const MAX_HISTORY_SLOTS: usize = 64;

/// The most encoded bytes the hierarchy lane holds before the connection
/// stops draining its hierarchy feed, so a slow client makes core's queue
/// lag instead of growing the lane.
pub const HIERARCHY_LANE_BUDGET: usize = 1 << 20;

/// The encoded bytes a connection retains, at which producers stop
/// admitting messages and the connection stops reading. Staying at it for
/// [`WRITER_STALL_DEADLINE`] disconnects the client.
pub const OUTBOUND_BUDGET: usize = 8 << 20;

/// How far above the lowest request ID the client has not used yet a
/// request may arrive. Request IDs start at 1 and are never reused; the
/// window keeps the record of used IDs bounded when a client skips one.
pub const REQUEST_ID_WINDOW: u64 = 1024;

/// The most viewers one connection holds, subscriptions in flight
/// included. Past it, `SubscribeTerminal` answers `LimitExceeded`.
pub const MAX_CONNECTION_VIEWERS: usize = 256;

/// How long a viewer's staged ordered messages wait before the connection
/// offers them to core again, the desktop's input retry interval. The
/// deadline is armed only while something is staged.
pub const STAGING_RETRY: std::time::Duration =
    std::time::Duration::from_millis(16);

/// The most ended viewers a connection remembers, so a request naming one
/// answers `Revoked` or `Stopped` instead of `StaleTarget`.
pub const MAX_ENDED_VIEWERS: usize = 256;

/// Limits a server applies to each connection. The defaults are the named
/// constants; tests lower them to reach a limit cheaply.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ServerConfig {
    /// The most requests a client may have outstanding, which `Welcome`
    /// advertises.
    pub max_in_flight_requests: u32,
    /// See [`HIERARCHY_LANE_BUDGET`].
    pub hierarchy_lane_budget: usize,
    /// See [`OUTBOUND_BUDGET`].
    pub outbound_budget: usize,
    /// See [`MAX_CONNECTION_VIEWERS`].
    pub max_viewers: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            max_in_flight_requests: MAX_IN_FLIGHT_REQUESTS,
            hierarchy_lane_budget: HIERARCHY_LANE_BUDGET,
            outbound_budget: OUTBOUND_BUDGET,
            max_viewers: MAX_CONNECTION_VIEWERS,
        }
    }
}

impl ServerConfig {
    /// The most encoded bytes one connection at negotiated caps `caps` ever
    /// retains: the outbound budget, one result or hierarchy message a
    /// producer admitted just below it, and the one replacement snapshot a
    /// connection may have in flight, which it admits whatever the queue
    /// holds.
    #[must_use]
    pub fn retained_bound(&self, caps: MessageCaps) -> usize {
        let cap = |kind| caps.cap(kind).unwrap_or(0);
        let hierarchy = cap(MessageKind::HierarchySnapshot)
            .max(cap(MessageKind::HierarchyResync))
            .max(cap(MessageKind::HierarchyEvent));
        self.outbound_budget
            + hierarchy
            + cap(MessageKind::Result).max(hierarchy)
    }
}
