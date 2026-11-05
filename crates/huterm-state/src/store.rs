//! One socket scope's saved files, read and written by the lock holder.

use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Read as _};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use huterm_protocol::RuntimeCapture;

use crate::bounds::MAX_FILE_BYTES;
use crate::paths::scope_directory;
pub use crate::schema::ScopeId;
use crate::schema::{
    Decoded, decode_desktop, decode_runtime, encode_desktop, encode_runtime,
};
use crate::{
    DesktopLayout, InvalidState, OpenError, SaveError, ScopeLock, atomic,
    valid_socket_name,
};

const LOCK_FILE: &str = "lock";
const RESTORE_ATTEMPT_FILE: &str = "restore-attempt";

/// A saved file a store reads and writes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StateFile {
    /// `runtime.json`, the runtime structure.
    Runtime,
    /// `desktop.json`, the desktop client's layout.
    Desktop,
}

impl StateFile {
    /// The file's name in the scope directory.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Runtime => "runtime.json",
            Self::Desktop => "desktop.json",
        }
    }

    /// The name a corrupt copy is set aside under.
    #[must_use]
    pub fn corrupt_name(self) -> &'static str {
        match self {
            Self::Runtime => "runtime.json.corrupt",
            Self::Desktop => "desktop.json.corrupt",
        }
    }
}

/// A runtime file that loaded, with its identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavedRuntime {
    /// The saved structure, with unscoped identities and no fallback pane
    /// names; a restore takes those from the commands it launches.
    pub capture: RuntimeCapture,
    /// The scope identity.
    pub scope_id: ScopeId,
    /// The save that wrote it.
    pub generation: u64,
}

/// What reading `runtime.json` found.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RuntimeLoad {
    /// No file.
    Missing,
    /// A valid file.
    Loaded(SavedRuntime),
    /// Unparseable, invalid, or over a bound, with the reason. Set it
    /// aside before the next save replaces it.
    Corrupt(String),
    /// Written by a newer version, or naming a newer feature. This store
    /// refuses to overwrite it for the rest of its life.
    Newer(String),
    /// The file exists but could not be read. Saves refuse to replace it.
    Unreadable(String),
}

/// What reading `desktop.json` found.
#[derive(Clone, Debug, PartialEq)]
pub enum DesktopLoad {
    /// No file.
    Missing,
    /// A valid layout saved with the loaded runtime file.
    Loaded(DesktopLayout),
    /// Unparseable, invalid, or over a bound, with the reason.
    Corrupt(String),
    /// Written by a newer version. Saves leave it in place for the rest of
    /// the store's life and write only the runtime file.
    Newer(String),
    /// A valid layout from another save than the runtime file's, or with
    /// no runtime file to name, so its identities cannot be trusted.
    Mismatched,
    /// The file exists but could not be read. Saves leave it in place.
    Unreadable(String),
}

/// Both files' load results.
#[derive(Clone, Debug, PartialEq)]
pub struct Loaded {
    /// `runtime.json`.
    pub runtime: RuntimeLoad,
    /// `desktop.json`, judged against the runtime file.
    pub desktop: DesktopLoad,
}

/// A locked socket scope's directory and saved files.
///
/// Opening takes the scope lock and holds it until the store drops, so
/// only one store per scope exists across processes.
#[derive(Debug)]
pub struct Store {
    directory: PathBuf,
    socket_name: String,
    /// The identity the last load or save recorded; the next save keeps
    /// its scope and writes the following generation.
    identity: Option<(ScopeId, u64)>,
    /// Whether a save may replace `runtime.json`.
    runtime_guard: RuntimeGuard,
    /// A load found `desktop.json` newer or unreadable, so saves leave it
    /// in place and it is never set aside.
    keep_desktop: bool,
    _lock: ScopeLock,
}

/// What a store knows about the `runtime.json` a save would replace.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RuntimeGuard {
    /// Not yet found missing or valid, nor set aside.
    Unresolved,
    /// Missing, valid, or set aside, so a save may replace it.
    Replaceable,
    /// Written by a newer version, so no save replaces it.
    Newer,
}

impl Store {
    /// Creates `root/sockets/<socket_name>/` (`0700`) as needed, takes its
    /// lock without waiting, and removes temporary files an interrupted
    /// write left.
    ///
    /// # Errors
    /// Rejects a socket name outside the socket grammar, reports a held
    /// lock, and reports directory or lock failures.
    pub fn open(root: &Path, socket_name: &str) -> Result<Self, OpenError> {
        if !valid_socket_name(socket_name) {
            return Err(OpenError::InvalidSocketName);
        }
        let directory = scope_directory(root, socket_name);
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&directory)
            .map_err(OpenError::Io)?;
        let lock = ScopeLock::acquire(&directory.join(LOCK_FILE))
            .map_err(OpenError::Lock)?;
        atomic::remove_temporaries(&directory).map_err(OpenError::Io)?;
        Ok(Self {
            directory,
            socket_name: socket_name.into(),
            identity: None,
            runtime_guard: RuntimeGuard::Unresolved,
            keep_desktop: false,
            _lock: lock,
        })
    }

    /// The scope directory.
    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Reads and classifies both files. A desktop layout counts only when
    /// its scope identity and generation match the runtime file's. Nothing
    /// is renamed or deleted.
    ///
    /// Saves wait for a load that finds `runtime.json` missing or valid. A
    /// newer runtime file, and a newer or unreadable desktop file, stay
    /// protected from saves and [`Self::set_aside`] for the rest of the
    /// store's life, whatever later loads find.
    pub fn load(&mut self) -> Loaded {
        let runtime = match self.read(StateFile::Runtime) {
            FileRead::Missing => RuntimeLoad::Missing,
            FileRead::Unreadable(error) => RuntimeLoad::Unreadable(error),
            FileRead::Oversize => RuntimeLoad::Corrupt(oversize()),
            FileRead::Bytes(bytes) => match decode_runtime(&bytes) {
                Decoded::Ok(file)
                    if file.content.socket_name != self.socket_name =>
                {
                    RuntimeLoad::Corrupt("the file names another socket".into())
                }
                Decoded::Ok(file) => RuntimeLoad::Loaded(SavedRuntime {
                    capture: file.content,
                    scope_id: file.scope_id,
                    generation: file.generation,
                }),
                Decoded::Corrupt(reason) => RuntimeLoad::Corrupt(reason),
                Decoded::Newer(reason) => RuntimeLoad::Newer(reason),
            },
        };
        if self.runtime_guard != RuntimeGuard::Newer {
            self.runtime_guard = match runtime {
                RuntimeLoad::Missing | RuntimeLoad::Loaded(_) => {
                    RuntimeGuard::Replaceable
                }
                RuntimeLoad::Newer(_) => RuntimeGuard::Newer,
                RuntimeLoad::Corrupt(_) | RuntimeLoad::Unreadable(_) => {
                    RuntimeGuard::Unresolved
                }
            };
        }
        self.identity = match &runtime {
            RuntimeLoad::Loaded(saved) => {
                Some((saved.scope_id, saved.generation))
            }
            _ => None,
        };
        let desktop = match self.read(StateFile::Desktop) {
            FileRead::Missing => DesktopLoad::Missing,
            FileRead::Unreadable(error) => DesktopLoad::Unreadable(error),
            FileRead::Oversize => DesktopLoad::Corrupt(oversize()),
            FileRead::Bytes(bytes) => match decode_desktop(&bytes) {
                Decoded::Ok(file)
                    if self.identity
                        == Some((file.scope_id, file.generation)) =>
                {
                    DesktopLoad::Loaded(file.content)
                }
                Decoded::Ok(_) => DesktopLoad::Mismatched,
                Decoded::Corrupt(reason) => DesktopLoad::Corrupt(reason),
                Decoded::Newer(reason) => DesktopLoad::Newer(reason),
            },
        };
        self.keep_desktop |= matches!(
            desktop,
            DesktopLoad::Newer(_) | DesktopLoad::Unreadable(_)
        );
        Loaded { runtime, desktop }
    }

    /// Writes `runtime.json`, then `desktop.json`, both with this scope's
    /// identity (a new one when no runtime file loaded) and the next
    /// generation, which it returns. Both are validated before either is
    /// written. A failure leaves every file not yet replaced as it was, so
    /// a layout from an earlier save then no longer matches. A newer or
    /// unreadable `desktop.json` is left in place and only `runtime.json`
    /// is written.
    ///
    /// # Errors
    /// Refuses a capture or layout over a bound or invalid, refuses until a
    /// load or [`Self::set_aside`] shows `runtime.json` may be replaced,
    /// and refuses to replace a newer runtime file; reports write failures.
    pub fn save(
        &mut self,
        capture: &RuntimeCapture,
        layout: &DesktopLayout,
    ) -> Result<u64, SaveError> {
        self.save_with(capture, layout, |_| Ok(()))
    }

    /// [`Self::save`] with `before_rename` run before each file replaces
    /// its target, so tests can interrupt a save between the two files.
    pub(crate) fn save_with(
        &mut self,
        capture: &RuntimeCapture,
        layout: &DesktopLayout,
        mut before_rename: impl FnMut(StateFile) -> io::Result<()>,
    ) -> Result<u64, SaveError> {
        match self.runtime_guard {
            RuntimeGuard::Unresolved => {
                return Err(SaveError::RuntimeUnresolved);
            }
            RuntimeGuard::Newer => return Err(SaveError::NewerVersion),
            RuntimeGuard::Replaceable => {}
        }
        if capture.socket_name != self.socket_name {
            return Err(
                InvalidState::new("the capture names another socket").into()
            );
        }
        let (scope_id, generation) = match self.identity {
            Some((scope_id, generation)) => (
                scope_id,
                generation.checked_add(1).ok_or_else(|| {
                    InvalidState::new("generations exhausted")
                })?,
            ),
            None => (ScopeId::generate()?, 1),
        };
        let runtime = encode_runtime(capture, scope_id, generation)?;
        let desktop = encode_desktop(layout, scope_id, generation)?;
        atomic::write_with(
            &self.directory,
            StateFile::Runtime.name(),
            &runtime,
            |_| before_rename(StateFile::Runtime),
        )?;
        self.identity = Some((scope_id, generation));
        if self.keep_desktop {
            return Ok(generation);
        }
        atomic::write_with(
            &self.directory,
            StateFile::Desktop.name(),
            &desktop,
            |_| before_rename(StateFile::Desktop),
        )?;
        Ok(generation)
    }

    /// Renames `file` to its `.corrupt` name, replacing an older copy. A
    /// missing file is left alone, and so is a file this store protects:
    /// a newer runtime file, or a newer or unreadable desktop file. Once a
    /// runtime file is set aside, the next save may write one, with a new
    /// scope identity.
    ///
    /// # Errors
    /// Reports a failed rename or directory sync.
    pub fn set_aside(&mut self, file: StateFile) -> io::Result<()> {
        let protected = match file {
            StateFile::Runtime => self.runtime_guard == RuntimeGuard::Newer,
            StateFile::Desktop => self.keep_desktop,
        };
        if protected {
            return Ok(());
        }
        match atomic::rename(&self.directory, file.name(), file.corrupt_name())
        {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            result => result?,
        }
        if file == StateFile::Runtime {
            self.identity = None;
            self.runtime_guard = RuntimeGuard::Replaceable;
        }
        Ok(())
    }

    /// Creates the `restore-attempt` marker and syncs it, so a crash during
    /// a restore leaves it behind.
    ///
    /// # Errors
    /// Reports a failed write or sync.
    pub fn mark_restore_attempt(&self) -> io::Result<()> {
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(self.directory.join(RESTORE_ATTEMPT_FILE))?
            .sync_all()?;
        atomic::sync_directory(&self.directory)
    }

    /// Whether a `restore-attempt` marker remains, which means the last
    /// restore never finished.
    ///
    /// # Errors
    /// Reports when the marker's existence cannot be checked.
    pub fn restore_attempt_pending(&self) -> io::Result<bool> {
        self.directory.join(RESTORE_ATTEMPT_FILE).try_exists()
    }

    /// Removes the `restore-attempt` marker, if present.
    ///
    /// # Errors
    /// Reports a failed removal or directory sync.
    pub fn clear_restore_attempt(&self) -> io::Result<()> {
        match fs::remove_file(self.directory.join(RESTORE_ATTEMPT_FILE)) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            result => {
                result?;
                atomic::sync_directory(&self.directory)
            }
        }
    }

    /// Reads a file, checking its size before reading the body.
    fn read(&self, file: StateFile) -> FileRead {
        let read = || -> io::Result<Option<Vec<u8>>> {
            let opened = File::open(self.directory.join(file.name()))?;
            if opened.metadata()?.len() > MAX_FILE_BYTES {
                return Ok(None);
            }
            let mut bytes = Vec::new();
            opened.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
            Ok((bytes.len() as u64 <= MAX_FILE_BYTES).then_some(bytes))
        };
        match read() {
            Ok(Some(bytes)) => FileRead::Bytes(bytes),
            Ok(None) => FileRead::Oversize,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                FileRead::Missing
            }
            Err(error) => FileRead::Unreadable(error.to_string()),
        }
    }
}

enum FileRead {
    Missing,
    Oversize,
    Unreadable(String),
    Bytes(Vec<u8>),
}

fn oversize() -> String {
    format!("the file is larger than {MAX_FILE_BYTES} bytes")
}

#[cfg(test)]
pub(crate) mod tests;
