//! A connection's close tickets.
//!
//! `PrepareClose` reserves a ticket when its job starts, and the ticket
//! keeps the assessment once the job completes. `CommitClose` takes the
//! assessment out while its job runs; the commit consumes the ticket
//! whatever the outcome, except `ConfirmationRequired`, which returns it so
//! the client can commit again with consent. A ticket whose record a
//! running job holds still counts against [`MAX_CLOSE_TICKETS`].

use std::collections::BTreeMap;

use huterm_wire::error::{ErrorCode, WireError};

use crate::config::MAX_CLOSE_TICKETS;
use crate::work::Consent;

#[derive(Debug)]
enum State {
    /// The prepare job runs.
    Preparing,
    /// The assessment waits for a commit or release.
    Ready(Box<Consent>),
    /// A commit job holds the assessment.
    Out,
}

#[derive(Debug)]
struct Entry {
    state: State,
    /// Released while a job held it: dropped when the job completes.
    released: bool,
}

#[derive(Debug, Default)]
pub(super) struct Tickets {
    entries: BTreeMap<u32, Entry>,
    last: u32,
}

impl Tickets {
    /// Reserves a ticket for a prepare job.
    pub(super) fn reserve(&mut self) -> Result<u32, WireError> {
        if self.entries.len() >= MAX_CLOSE_TICKETS {
            return Err(WireError::new(
                ErrorCode::LimitExceeded,
                format!(
                    "a connection holds at most {MAX_CLOSE_TICKETS} close tickets"
                ),
            ));
        }
        let ticket = self.last.checked_add(1).ok_or_else(|| {
            WireError::new(ErrorCode::LimitExceeded, "close tickets exhausted")
        })?;
        self.last = ticket;
        self.entries.insert(
            ticket,
            Entry {
                state: State::Preparing,
                released: false,
            },
        );
        Ok(ticket)
    }

    /// Keeps a prepared assessment; false when the ticket was released
    /// meanwhile, which drops it.
    pub(super) fn prepared(&mut self, ticket: u32, consent: Consent) -> bool {
        match self.entries.get_mut(&ticket) {
            Some(entry) if !entry.released => {
                entry.state = State::Ready(Box::new(consent));
                true
            }
            _ => {
                self.entries.remove(&ticket);
                false
            }
        }
    }

    /// Forgets a ticket whose prepare failed.
    pub(super) fn forget(&mut self, ticket: u32) {
        self.entries.remove(&ticket);
    }

    /// Takes a ticket's assessment for a commit job.
    pub(super) fn take(&mut self, ticket: u32) -> Result<Consent, WireError> {
        let entry = self.entries.get_mut(&ticket).ok_or_else(|| {
            WireError::new(
                ErrorCode::StaleTarget,
                format!("no close ticket {ticket}"),
            )
        })?;
        match std::mem::replace(&mut entry.state, State::Out) {
            State::Ready(consent) => Ok(*consent),
            busy => {
                entry.state = busy;
                Err(WireError::new(
                    ErrorCode::Busy,
                    format!("close ticket {ticket} is in use"),
                ))
            }
        }
    }

    /// Ends a commit: the ticket stays with `consent` when the commit
    /// needs confirmation, and is consumed otherwise.
    pub(super) fn committed(&mut self, ticket: u32, consent: Option<Consent>) {
        match (self.entries.get_mut(&ticket), consent) {
            (Some(entry), Some(consent)) if !entry.released => {
                entry.state = State::Ready(Box::new(consent));
            }
            _ => {
                self.entries.remove(&ticket);
            }
        }
    }

    /// Releases a ticket without committing it.
    pub(super) fn release(&mut self, ticket: u32) -> Result<(), WireError> {
        let entry = self.entries.get_mut(&ticket).ok_or_else(|| {
            WireError::new(
                ErrorCode::StaleTarget,
                format!("no close ticket {ticket}"),
            )
        })?;
        if matches!(entry.state, State::Ready(_)) {
            self.entries.remove(&ticket);
        } else {
            entry.released = true;
        }
        Ok(())
    }

    /// Drops every ticket.
    pub(super) fn clear(&mut self) {
        self.entries.clear();
    }
}
