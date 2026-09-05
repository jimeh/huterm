//! Terminal runtime, emulator, PTY, and multiplexer ownership.

#![deny(missing_docs)]

mod engine;
mod input;
mod mux;
mod pty;
mod terminal;

pub use mux::{
    DEFAULT_SOCKET_NAME, Mux, MuxError, OpenedTab, SelectionTarget, Session,
    Tab, Workspace,
};
pub use terminal::{
    RuntimeClient, RuntimeError, SelectionRequest, SnapshotReply,
    SnapshotRequest, TerminalRuntime,
};

/// Immutable native revision used by the optional Ghostty adapter.
pub const GHOSTTY_REVISION: &str = "a887df42c56f6de86c0fe6da9c4eeca37931e083";
