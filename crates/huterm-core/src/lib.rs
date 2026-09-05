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
