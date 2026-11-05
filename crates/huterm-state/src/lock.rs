//! The scope lock, which makes one process the scope's only reader and
//! writer.

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use nix::errno::Errno;
use nix::fcntl::{Flock, FlockArg};

/// An exclusive `flock` on a scope's `lock` file, held until dropped. The
/// kernel also releases it when the process exits, so no stale lock needs
/// cleaning up.
#[derive(Debug)]
pub struct ScopeLock {
    _lock: Flock<File>,
}

/// Why the scope lock was not taken.
#[derive(Debug)]
pub enum LockError {
    /// Another open file description, usually another process, holds it.
    Held,
    /// The lock file could not be opened or locked.
    Io(io::Error),
}

impl fmt::Display for LockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Held => f.write_str("another process holds the state lock"),
            Self::Io(error) => write!(f, "could not lock saved state: {error}"),
        }
    }
}

impl std::error::Error for LockError {}

impl ScopeLock {
    /// Takes `flock(LOCK_EX | LOCK_NB)` on `path`, creating it `0600` when
    /// missing, without waiting.
    ///
    /// # Errors
    /// Returns [`LockError::Held`] when the lock is taken and
    /// [`LockError::Io`] for any other failure.
    pub fn acquire(path: &Path) -> Result<Self, LockError> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path)
            .map_err(LockError::Io)?;
        match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
            Ok(lock) => Ok(Self { _lock: lock }),
            Err((_, Errno::EWOULDBLOCK)) => Err(LockError::Held),
            Err((_, errno)) => Err(LockError::Io(errno.into())),
        }
    }
}
