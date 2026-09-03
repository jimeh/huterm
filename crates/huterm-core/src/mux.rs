use std::collections::{HashMap, hash_map::Entry};

use huterm_protocol::{PaneId, SessionId, TabId, TerminalId};
use thiserror::Error;

/// Full ownership chain for one terminal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TerminalOwner {
    /// Owning session.
    pub session_id: SessionId,
    /// Owning tab.
    pub tab_id: TabId,
    /// Owning pane.
    pub pane_id: PaneId,
}

/// Minimal one-terminal mux model used by the proof of concept.
#[derive(Debug, Default)]
pub struct Mux {
    terminals: HashMap<TerminalId, TerminalOwner>,
}

impl Mux {
    /// Inserts a terminal and its complete ownership chain.
    ///
    /// # Errors
    ///
    /// Returns [`MuxError::DuplicateTerminal`] when the ID already exists.
    pub fn insert(
        &mut self,
        terminal_id: TerminalId,
        owner: TerminalOwner,
    ) -> Result<(), MuxError> {
        match self.terminals.entry(terminal_id) {
            Entry::Vacant(entry) => {
                entry.insert(owner);
            }
            Entry::Occupied(_) => {
                return Err(MuxError::DuplicateTerminal(terminal_id));
            }
        }
        Ok(())
    }

    /// Looks up the ownership chain for a terminal.
    #[must_use]
    pub fn owner(&self, terminal_id: TerminalId) -> Option<TerminalOwner> {
        self.terminals.get(&terminal_id).copied()
    }

    /// Removes a terminal from its owning pane.
    ///
    /// # Errors
    ///
    /// Returns [`MuxError::UnknownTerminal`] when the ID is absent.
    pub fn close(
        &mut self,
        terminal_id: TerminalId,
    ) -> Result<TerminalOwner, MuxError> {
        self.terminals
            .remove(&terminal_id)
            .ok_or(MuxError::UnknownTerminal(terminal_id))
    }

    /// Returns the number of live terminals.
    #[must_use]
    pub fn terminal_count(&self) -> usize {
        self.terminals.len()
    }
}

/// Mux ownership error.
#[derive(Debug, Error, Eq, PartialEq)]
pub enum MuxError {
    /// A terminal ID was inserted twice.
    #[error("terminal {0:?} already exists")]
    DuplicateTerminal(TerminalId),
    /// The requested terminal does not exist.
    #[error("terminal {0:?} does not exist")]
    UnknownTerminal(TerminalId),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner() -> TerminalOwner {
        TerminalOwner {
            session_id: SessionId::new(1),
            tab_id: TabId::new(2),
            pane_id: PaneId::new(3),
        }
    }

    #[test]
    fn close_should_remove_terminal_from_ownership_chain() {
        let terminal_id = TerminalId::new(4);
        let mut mux = Mux::default();
        mux.insert(terminal_id, owner())
            .expect("insert should succeed");

        let closed = mux.close(terminal_id).expect("close should succeed");

        assert_eq!((closed, mux.terminal_count()), (owner(), 0));
    }

    #[test]
    fn insert_should_reject_duplicate_terminal_id() {
        let terminal_id = TerminalId::new(4);
        let mut mux = Mux::default();
        mux.insert(terminal_id, owner())
            .expect("insert should succeed");

        let replacement = TerminalOwner {
            session_id: SessionId::new(9),
            tab_id: TabId::new(8),
            pane_id: PaneId::new(7),
        };
        let error = mux
            .insert(terminal_id, replacement)
            .expect_err("duplicate should fail");

        assert_eq!(error, MuxError::DuplicateTerminal(terminal_id));
        assert_eq!(mux.owner(terminal_id), Some(owner()));
    }
}
