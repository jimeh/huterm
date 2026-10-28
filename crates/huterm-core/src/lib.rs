//! Terminal runtime, emulator, PTY, and multiplexer ownership.

#![deny(missing_docs)]

#[cfg(unix)]
mod child_wait;
mod commands;
mod compression;
pub use commands::execute;
pub use compression::CompressionStats;
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

/// Whether this build can compress idle scrollback: true on every 64-bit
/// target Huterm builds for.
pub const SCROLLBACK_COMPRESSION_SUPPORTED: bool =
    huterm_ghostty::COMPRESSION_SUPPORTED;

/// Immutable native revision used by the Ghostty adapter.
pub const GHOSTTY_REVISION: &str = "83fb95624b0439ee660c2679b43b38c8b63769cd";
