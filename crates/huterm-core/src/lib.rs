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
    Beside, CloseAssessment, CloseEffect, CloseRequest, CloseTicket,
    DEFAULT_SOCKET_NAME, HierarchyRecvError, HierarchySnapshot,
    HierarchySubscription, MoveDestination, Mux, MuxError, OpenedPane,
    OpenedTab, PaneRef, RestoreDirection, SelectionTarget, Session, Tab,
    Workspace,
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
pub const GHOSTTY_REVISION: &str = "83fb95624b0439ee660c2679b43b38c8b63769cd";
