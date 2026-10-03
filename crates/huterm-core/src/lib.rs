//! Terminal runtime, emulator, PTY, and multiplexer ownership.

#![deny(missing_docs)]

#[cfg(unix)]
mod child_wait;
mod commands;
pub use commands::execute;
mod engine;
mod foreground;
mod host_effects;
mod input;
mod jobs;
mod limits;
pub use host_effects::{
    DesktopHostEffectClient, HostEffectClientOrigin, HostEffectRecipient,
    HostEffectRecipientOptions, PendingHostEffect,
};
pub use jobs::{COMMAND_LINE_MAX_CHARS, JobProcess, JobState};
pub use limits::raise_open_file_limit;
mod mux;
mod pty;
mod terminal;
#[cfg(test)]
mod test_support;
mod viewer;
mod wake;

pub use mux::{
    CloseAssessment, CloseEffect, CloseRequest, CloseTicket,
    DEFAULT_SOCKET_NAME, HierarchyRecvError, HierarchySnapshot,
    HierarchySubscription, Mux, MuxError, OpenedTab, SelectionTarget, Session,
    Tab, Workspace,
};
pub use terminal::{
    RefusedInput, RuntimeError, SelectionRequest, SnapshotReply,
    SnapshotRequest, TerminalRuntime,
};
pub use viewer::{
    HostEffectViewerOptions, TerminalViewer, ViewerOptions, ViewerUpdate,
    ViewerWake,
};

/// Immutable native revision used by the Ghostty adapter.
pub const GHOSTTY_REVISION: &str = "56dbc4a768778753737a3b9cbe0a3f9b4e434553";
