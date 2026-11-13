//! What a client brings to every connection: its `Hello` and the deadlines
//! of its own transport profile.
//!
//! The server's `Welcome` advertises the limits that bind the client, such
//! as in-flight requests and message caps. The two deadlines here bound
//! how long the client waits on the server, so they belong to the client:
//! local IPC uses the constants, and tests shorten them.

use std::time::Duration;

use huterm_wire::handshake::{ClientKind, Hello};

/// How long a client waits for `Welcome` or `Reject` on local IPC, well
/// beyond the link's round trip. A hung server cannot hold a connection
/// that never completes its handshake.
pub const CLIENT_HANDSHAKE_DEADLINE: Duration = Duration::from_secs(5);

/// How long a client waits for any frame on local IPC while a request,
/// a sequence waiter, or a subscription result waiting for the projection
/// is outstanding. Every inbound frame restarts it, and nothing
/// outstanding arms nothing.
pub const CLIENT_PROGRESS_DEADLINE: Duration = Duration::from_secs(30);

/// A client's configuration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClientConfig {
    /// The `Hello` every connection opens with.
    pub hello: Hello,
    /// See [`CLIENT_HANDSHAKE_DEADLINE`].
    pub handshake_deadline: Duration,
    /// See [`CLIENT_PROGRESS_DEADLINE`].
    pub progress_deadline: Duration,
}

impl ClientConfig {
    /// A local IPC client at this build's versions.
    #[must_use]
    pub fn local(kind: ClientKind, name: &str, build_version: &str) -> Self {
        Self {
            hello: Hello::new(kind, name, build_version),
            handshake_deadline: CLIENT_HANDSHAKE_DEADLINE,
            progress_deadline: CLIENT_PROGRESS_DEADLINE,
        }
    }
}
