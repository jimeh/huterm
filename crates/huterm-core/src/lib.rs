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
