//! Huterm's runtime IPC server: the per-connection protocol over a `Mux`.
//!
//! A [`Connection`] is the whole protocol of one client connection as a
//! deterministic, single-threaded state machine over bytes, values, and an
//! injected clock. It never touches the `Mux`: it hands out [`Work`] that
//! [`runtime::execute`] runs against the shared `Mux` on a worker, and takes
//! back each [`Completion`]. The native adapter in [`native`] drives one
//! connection over a `UnixStream` with reader, writer, and driver threads,
//! and [`listener::Listener`] accepts connections on a Unix socket whose
//! path, permissions, and peer credentials `huterm-socket` checks.
//!
//! See `docs/plans/runtime-ipc.md`; #45 adds the daemon.

#![deny(missing_docs)]

#[cfg(test)]
mod acceptance;
#[cfg(all(test, unix))]
mod benchmark;
pub mod config;
pub mod connection;
#[cfg(all(test, unix))]
mod contract_tests;
mod convert;
#[cfg(test)]
mod effect_acceptance;
#[cfg(all(test, unix))]
mod effect_tests;
#[cfg(test)]
mod faults;
pub mod feed;
#[cfg(unix)]
pub mod listener;
#[cfg(any(test, feature = "test-support"))]
pub mod model;
#[cfg(unix)]
pub mod native;
pub mod runtime;
mod server;
#[cfg(any(test, feature = "test-minor"))]
pub mod test_feed;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod viewer;
#[cfg(all(test, unix))]
mod viewer_tests;
pub mod work;
#[cfg(test)]
mod world;

pub use config::ServerConfig;
pub use connection::{CloseReason, Connection, Transmit};
#[cfg(unix)]
pub use listener::Listener;
pub use server::Server;
pub use work::{Completion, Job, Work, WorkId};
