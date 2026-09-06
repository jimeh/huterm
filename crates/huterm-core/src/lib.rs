//! Terminal runtime, emulator, PTY, and multiplexer ownership.

#![deny(missing_docs)]

mod engine;
mod input;
mod jobs;
pub use jobs::{JobProcess, JobState};
mod mux;
mod pty;
mod terminal;

pub use mux::{
    CloseAssessment, CloseEffect, CloseRequest, CloseTicket,
    DEFAULT_SOCKET_NAME, HierarchySnapshot, Mux, MuxError, OpenedTab,
    SelectionTarget, Session, Tab, Workspace,
};
pub use terminal::{
    RuntimeClient, RuntimeError, SelectionRequest, SnapshotReply,
    SnapshotRequest, TerminalRuntime,
};

/// Immutable native revision used by the Ghostty adapter.
pub const GHOSTTY_REVISION: &str = "a887df42c56f6de86c0fe6da9c4eeca37931e083";
