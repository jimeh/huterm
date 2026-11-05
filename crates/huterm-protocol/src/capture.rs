//! A runtime's restorable structure, captured without processes.
//!
//! A [`RuntimeCapture`] records what a restore recreates: sessions,
//! workspaces, and tabs in order with their identities, names, and pane
//! layouts, the allocator watermark and creation ordinals, and for each
//! terminal the directories its fresh shell may start in. It records no
//! program, argument, environment, title, process, command record, or exit
//! state, so restoring it never runs saved text.
//!
//! Identities in a capture are unscoped ([`RuntimeId`] zero): the numeric
//! values survive, and a restore gives them the restoring runtime's scope
//! (see [`crate::RuntimeId`]).

use std::collections::HashSet;
use std::fmt;
use std::path::PathBuf;

use crate::{
    PaneLayout, SessionInfo, TabId, TabInfo, TerminalId, WorkspaceInfo,
};

/// The most directory candidates a terminal capture holds.
pub const MAX_DIRECTORY_CANDIDATES: usize = 3;

/// A runtime's sessions, workspaces, tabs, and terminals, in order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeCapture {
    /// Logical socket name of the captured runtime.
    pub socket_name: String,
    /// The allocator watermark: every captured identity is at or below it,
    /// and a restored runtime allocates above it.
    pub next_id: u64,
    /// The last session creation ordinal, which automatic names count.
    pub session_ordinal: u64,
    /// The last workspace creation ordinal, which automatic names count.
    pub workspace_ordinal: u64,
    /// Sessions in order, each with its workspaces and their tabs.
    pub sessions: Vec<SessionCapture>,
    /// One record for each terminal a captured pane shows, in pane
    /// traversal order.
    pub terminals: Vec<TerminalCapture>,
}

/// One session and its workspaces in order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionCapture {
    /// Identity and names.
    pub info: SessionInfo,
    /// Workspaces in order.
    pub workspaces: Vec<WorkspaceCapture>,
}

/// One workspace and its tabs in order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceCapture {
    /// Identity and names.
    pub info: WorkspaceInfo,
    /// Tabs in order, with their layouts, shared zoom, and input sync.
    pub tabs: Vec<TabInfo>,
}

/// What a restore needs to start one terminal again.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalCapture {
    /// The terminal a pane shows.
    pub id: TerminalId,
    /// Directories to start in, most preferred first, without repeats: the
    /// effective local directory, the root shell's directory, and the
    /// launch directory. At most [`MAX_DIRECTORY_CANDIDATES`].
    pub directories: Vec<PathBuf>,
    /// What runs in the terminal.
    pub launch: TerminalLaunch,
}

/// What a restored terminal runs.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum TerminalLaunch {
    /// The shell the restoring client currently configures.
    DefaultShell,
}

/// Why a capture cannot be restored.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum CaptureError {
    /// An identity is zero, which no allocation produces.
    ZeroId,
    /// A numeric identity repeats, within or across kinds.
    DuplicateId(u64),
    /// An identity lies above the allocator watermark.
    AboveWatermark(u64),
    /// A pane shows a terminal without a terminal record.
    MissingTerminal(TerminalId),
    /// A terminal record names a terminal no pane shows.
    OrphanTerminal(TerminalId),
    /// A tab's shared zoom names no pane of a layout with several panes.
    InvalidZoom(TabId),
    /// A custom or automatic name has no visible characters.
    BlankName,
    /// A terminal record holds more than [`MAX_DIRECTORY_CANDIDATES`]
    /// directories, or repeats one.
    InvalidDirectories(TerminalId),
}

impl fmt::Display for CaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroId => f.write_str("an identity is zero"),
            Self::DuplicateId(id) => write!(f, "identity {id} repeats"),
            Self::AboveWatermark(id) => {
                write!(f, "identity {id} is above the allocator watermark")
            }
            Self::MissingTerminal(id) => {
                write!(f, "terminal {} has no record", id.get())
            }
            Self::OrphanTerminal(id) => {
                write!(f, "terminal {} belongs to no pane", id.get())
            }
            Self::InvalidZoom(tab) => {
                write!(f, "tab {} zooms a pane it cannot", tab.get())
            }
            Self::BlankName => f.write_str("a name is blank"),
            Self::InvalidDirectories(id) => {
                write!(f, "terminal {} has invalid directories", id.get())
            }
        }
    }
}

impl std::error::Error for CaptureError {}

impl RuntimeCapture {
    /// Every captured tab, in order.
    pub fn tabs(&self) -> impl Iterator<Item = &TabInfo> {
        self.sessions
            .iter()
            .flat_map(|session| &session.workspaces)
            .flat_map(|workspace| &workspace.tabs)
    }

    /// Looks up a terminal's record.
    #[must_use]
    pub fn terminal(&self, id: TerminalId) -> Option<&TerminalCapture> {
        self.terminals.iter().find(|terminal| terminal.id == id)
    }

    /// Checks every rule a restore relies on, all or nothing: nonzero
    /// identities, unique across sessions, workspaces, tabs, panes, splits,
    /// and terminals, none above the watermark; one terminal record for
    /// each pane's terminal and none other; a valid shared zoom; non-blank
    /// names; and bounded, distinct directory candidates. Layouts uphold
    /// their own invariants by construction.
    ///
    /// # Errors
    /// Returns the first rule the capture breaks.
    pub fn validate(&self) -> Result<(), CaptureError> {
        let mut ids = IdCheck {
            watermark: self.next_id,
            seen: HashSet::new(),
        };
        let mut shown = HashSet::new();
        for session in &self.sessions {
            ids.add(session.info.id.get())?;
            names(
                session.info.custom_name.as_deref(),
                &session.info.automatic_name,
            )?;
            for workspace in &session.workspaces {
                ids.add(workspace.info.id.get())?;
                names(
                    workspace.info.custom_name.as_deref(),
                    &workspace.info.automatic_name,
                )?;
                for tab in &workspace.tabs {
                    ids.add(tab.id.get())?;
                    if tab.custom_name.as_deref().is_some_and(blank) {
                        return Err(CaptureError::BlankName);
                    }
                    layout_ids(&tab.layout, &mut ids, &mut shown)?;
                    if tab.zoomed.is_some_and(|pane| {
                        tab.layout.pane_count() < 2
                            || tab.layout.pane(pane).is_none()
                    }) {
                        return Err(CaptureError::InvalidZoom(tab.id));
                    }
                }
            }
        }
        let mut recorded = HashSet::new();
        for terminal in &self.terminals {
            if !shown.contains(&terminal.id) {
                return Err(CaptureError::OrphanTerminal(terminal.id));
            }
            if !recorded.insert(terminal.id) {
                return Err(CaptureError::DuplicateId(terminal.id.get()));
            }
            let distinct: HashSet<_> = terminal.directories.iter().collect();
            if terminal.directories.len() > MAX_DIRECTORY_CANDIDATES
                || distinct.len() != terminal.directories.len()
            {
                return Err(CaptureError::InvalidDirectories(terminal.id));
            }
        }
        if let Some(missing) = shown.iter().find(|id| !recorded.contains(id)) {
            return Err(CaptureError::MissingTerminal(*missing));
        }
        Ok(())
    }
}

struct IdCheck {
    watermark: u64,
    seen: HashSet<u64>,
}

impl IdCheck {
    fn add(&mut self, id: u64) -> Result<(), CaptureError> {
        if id == 0 {
            return Err(CaptureError::ZeroId);
        }
        if id > self.watermark {
            return Err(CaptureError::AboveWatermark(id));
        }
        if !self.seen.insert(id) {
            return Err(CaptureError::DuplicateId(id));
        }
        Ok(())
    }
}

fn layout_ids(
    layout: &PaneLayout,
    ids: &mut IdCheck,
    shown: &mut HashSet<TerminalId>,
) -> Result<(), CaptureError> {
    for split in layout.split_ids() {
        ids.add(split.get())?;
    }
    for pane in layout.panes() {
        ids.add(pane.id.get())?;
        ids.add(pane.terminal_id.get())?;
        if pane.custom_name.as_deref().is_some_and(blank) {
            return Err(CaptureError::BlankName);
        }
        shown.insert(pane.terminal_id);
    }
    Ok(())
}

fn names(custom: Option<&str>, automatic: &str) -> Result<(), CaptureError> {
    if custom.is_some_and(blank) || blank(automatic) {
        Err(CaptureError::BlankName)
    } else {
        Ok(())
    }
}

fn blank(name: &str) -> bool {
    name.trim().is_empty()
}

#[cfg(test)]
mod tests;
