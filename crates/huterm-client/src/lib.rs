//! Huterm's runtime IPC client.
//!
//! [`ClientSession`] is the client's whole protocol as a deterministic,
//! sans-IO state machine over bytes, values, and an injected clock. It
//! lives across connections: it maps each server incarnation to a
//! client-local runtime, follows the hierarchy with a projection whose
//! digests it checks, parks sequence waiters on it, and applies the retry
//! rules when a connection breaks. It builds for `wasm32-unknown-unknown`.
//! On Unix, [`native`] runs a session over a `UnixStream` with reader,
//! writer, and driver threads behind async handles.
//!
//! See `docs/plans/runtime-ipc.md`. Slice 5 adds `connect`, which resolves
//! socket paths.

#![deny(missing_docs)]

pub mod config;
#[cfg(unix)]
pub mod native;
mod outbound;
mod projection;
pub mod session;

pub use config::{
    CLIENT_HANDSHAKE_DEADLINE, CLIENT_PROGRESS_DEADLINE, ClientConfig,
};
pub use outbound::{FRAME_HEADER_LEN, Transmit};
pub use session::{
    CallId, ClientEvent, ClientSession, CloseReason, Connected,
    HierarchyUpdate, Resolution, SessionStats, WaitId,
};
