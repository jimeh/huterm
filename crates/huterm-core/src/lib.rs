//! Terminal runtime, emulator, PTY, and multiplexer ownership.

#![deny(missing_docs)]

#[cfg(unix)]
mod child_wait;
mod commands;
pub use commands::execute;
mod engine;
mod events;
mod foreground;
mod host_effects;
mod input;
mod jobs;
mod limits;
pub use host_effects::{
    DesktopHostEffectClient, HostEffectClientOrigin, HostEffectRecipient,
    HostEffectRecipientOptions, PendingHostEffect,
};
pub use jobs::{JobProcess, JobState};
pub use limits::raise_open_file_limit;
mod mux;
mod presentation;
mod pty;
mod terminal;
mod wake;

pub use mux::{
    CloseAssessment, CloseEffect, CloseRequest, CloseTicket,
    DEFAULT_SOCKET_NAME, HierarchySnapshot, Mux, MuxError, OpenedTab,
    SelectionTarget, Session, Tab, Workspace,
};
pub use presentation::PresentationController;
pub use terminal::{
    RuntimeClient, RuntimeError, SelectionRequest, SnapshotReply,
    SnapshotRequest, TerminalRuntime,
};

/// Immutable native revision used by the Ghostty adapter.
pub const GHOSTTY_REVISION: &str = "22d13172cde98a0a4dda05d3d6a3fcb0dd8ed018";
