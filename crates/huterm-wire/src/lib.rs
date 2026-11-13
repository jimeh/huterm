//! Huterm's runtime IPC wire contract: the preamble, frames and their
//! streams, message caps, the codec, version and feature negotiation, wire
//! errors, and the encodings of hierarchy state and events, pane layouts,
//! catalog invocations, terminal updates and status, and viewer input.
//!
//! Everything here is sans-IO: pure functions and state machines over bytes
//! and values, so a native socket adapter and a browser's WebSocket callbacks
//! drive the same code. The crate depends only on `huterm-protocol` and
//! builds for `wasm32-unknown-unknown`. See `docs/plans/runtime-ipc.md`.
//!
//! Encoders destructure each protocol struct exhaustively, so a new field
//! fails to compile here until its wire form is decided. Decoders build
//! values through the protocol's validating constructors.

#![deny(missing_docs)]

pub mod caps;
pub mod codec;
pub mod command;
pub mod error;
pub mod frame;
pub mod handshake;
pub mod hierarchy;
pub mod message;
pub mod status;
pub mod stream;
#[cfg(any(test, feature = "test-minor"))]
pub mod test_minor;
pub mod update;
#[cfg(any(test, feature = "vectors"))]
pub mod vectors;
pub mod viewer;

use huterm_protocol::DigestProfile;

/// The protocol major version. A peer of another major version is
/// rejected; only a change an older peer cannot do without bumps it.
pub const MAJOR: u16 = 1;

/// The highest minor version this build speaks. A peer speaks every minor
/// version of its major from 0 up to its own, so a connection negotiates
/// the smaller of the two peers' values.
pub const MINOR: u16 = 0;

/// The digest profile for peers at `minor`: the hierarchy and terminal
/// fields that version holds. A minor version that adds digested state maps
/// to a profile that covers it.
#[must_use]
pub const fn digest_profile(minor: u16) -> DigestProfile {
    let _ = minor;
    DigestProfile::Base
}
