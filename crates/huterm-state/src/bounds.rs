//! Size bounds every saved file meets. The writer refuses a capture that
//! exceeds one, and the reader rejects a file that does, so a file one
//! build writes is one every build with the same bounds can read.

/// The largest file the reader opens, checked before reading its body.
pub const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
/// Sessions in a runtime file.
pub const MAX_SESSIONS: usize = 256;
/// Workspaces across all sessions.
pub const MAX_WORKSPACES: usize = 1024;
/// Tabs across all workspaces.
pub const MAX_TABS: usize = 4096;
/// Panes across all tabs, and so terminal records.
pub const MAX_PANES: usize = 16_384;
/// Nodes on the longest root-to-pane path of one tab's layout.
pub const MAX_LAYOUT_DEPTH: usize = 32;
/// Bytes in one saved directory path. Directory candidates are optional, so
/// the writer drops a longer one instead of refusing the capture.
pub const MAX_PATH_BYTES: usize = 4096;
/// Windows in a desktop layout.
pub const MAX_WINDOWS: usize = MAX_SESSIONS;
