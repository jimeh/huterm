//! Huterm's runtime IPC server: the per-connection protocol over a `Mux`.
//!
//! A [`Connection`] is the whole protocol of one client connection as a
//! deterministic, single-threaded state machine over bytes, values, and an
//! injected clock. It never touches the `Mux`: it hands out [`Work`] that
//! [`runtime::execute`] runs against the shared `Mux` on a worker, and takes
//! back each [`Completion`]. The native adapter in [`native`] drives one
//! connection over a `UnixStream` with reader, writer, and driver threads.
//!
//! See `docs/plans/runtime-ipc.md`. Slice 5 adds the listener, socket paths,
//! and peer credentials; #45 adds the daemon.

#![deny(missing_docs)]

#[cfg(all(test, unix))]
mod benchmark;
pub mod config;
pub mod connection;
#[cfg(all(test, unix))]
mod contract_tests;
mod convert;
pub mod feed;
#[cfg(unix)]
pub mod native;
pub mod runtime;
mod server;
#[cfg(any(test, feature = "test-minor"))]
pub mod test_feed;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod work;

pub use config::ServerConfig;
pub use connection::{CloseReason, Connection, Transmit};
pub use server::Server;
pub use work::{Completion, Job, Work, WorkId};
