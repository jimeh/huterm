//! Errors from opening, reading, and writing saved state.

use std::fmt;
use std::io;

use crate::LockError;

/// Why saved content breaks the schema, a bound, or a structural rule.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InvalidState(pub(crate) String);

impl InvalidState {
    pub(crate) fn new(reason: impl Into<String>) -> Self {
        Self(reason.into())
    }

    /// The reason, for a notice.
    #[must_use]
    pub fn reason(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for InvalidState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for InvalidState {}

/// Why a scope could not be opened.
#[derive(Debug)]
pub enum OpenError {
    /// The socket name is outside the socket grammar, which disables
    /// persistence for the scope.
    InvalidSocketName,
    /// The scope directory could not be created or cleaned.
    Io(io::Error),
    /// The scope lock is held or could not be taken.
    Lock(LockError),
}

impl fmt::Display for OpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSocketName => {
                f.write_str("the socket name cannot name saved state")
            }
            Self::Io(error) => {
                write!(f, "could not create the state directory: {error}")
            }
            Self::Lock(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for OpenError {}

/// Why a save wrote nothing, or not both files.
#[derive(Debug)]
pub enum SaveError {
    /// The capture or layout breaks a bound or rule; nothing was written.
    Invalid(InvalidState),
    /// The runtime file on disk came from a newer version, which this
    /// build never overwrites.
    NewerVersion,
    /// No load has found `runtime.json` missing or valid, and no corrupt
    /// one was set aside, so this store cannot tell that replacing it is
    /// safe.
    RuntimeUnresolved,
    /// Writing failed; files not yet replaced keep their previous content.
    Io(io::Error),
}

impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(error) => write!(f, "not saved: {error}"),
            Self::NewerVersion => {
                f.write_str("saved state came from a newer Huterm")
            }
            Self::RuntimeUnresolved => f.write_str(
                "the saved runtime file has not been loaded or set aside",
            ),
            Self::Io(error) => write!(f, "could not save: {error}"),
        }
    }
}

impl std::error::Error for SaveError {}

impl From<io::Error> for SaveError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<InvalidState> for SaveError {
    fn from(error: InvalidState) -> Self {
        Self::Invalid(error)
    }
}
