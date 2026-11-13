//! Where Huterm's runtime IPC sockets live, who may use them, and who is
//! on the other end.
//!
//! A server and its clients resolve the same [`SocketPath`] for a socket
//! name: an explicit path, else `HUTERM_SOCKET_DIR/<name>`, else, on Linux,
//! `$XDG_RUNTIME_DIR/huterm/<name>` when that directory is absolute and owned
//! by the effective user, else `/tmp/huterm-<uid>/<name>`. The socket's
//! directory must be private: a real directory, not a symlink, owned by the
//! effective user, with no group or other permissions. [`bind`] creates it
//! with mode `0700` when it is missing and refuses to listen in one that
//! fails the check; [`connect`] refuses to connect through one, so a client
//! never talks to a socket another user planted. Both ends then compare the
//! peer's user ID with their own effective user ID ([`check_peer`]).
//!
//! Stale sockets and startup races belong to the daemon (#45). See
//! `docs/plans/runtime-ipc.md`.

#![cfg(unix)]
#![deny(missing_docs)]

use std::ffi::OsString;
use std::fmt;
use std::fs::{self, DirBuilder, Permissions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

/// The socket name a server and its clients use when none is given.
pub const DEFAULT_SOCKET_NAME: &str = "default";

/// The environment variable that overrides the socket directory.
pub const SOCKET_DIR_VARIABLE: &str = "HUTERM_SOCKET_DIR";

/// The longest socket name.
pub const MAX_SOCKET_NAME_BYTES: usize = 64;

/// The size of `sockaddr_un.sun_path`, its terminating NUL included.
#[cfg(any(target_os = "linux", target_os = "android"))]
const SUN_PATH_BYTES: usize = 108;
#[cfg(not(any(target_os = "linux", target_os = "android")))]
const SUN_PATH_BYTES: usize = 104;

/// The longest socket path, in bytes, that fits `sun_path` with its NUL.
pub const MAX_SOCKET_PATH_BYTES: usize = SUN_PATH_BYTES - 1;

/// The mode [`bind`] creates a missing socket directory with.
const DIRECTORY_MODE: u32 = 0o700;

/// The mode [`bind`] gives the socket file.
const SOCKET_MODE: u32 = 0o600;

/// Why a socket directory is not private to the effective user.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirectoryProblem {
    /// The path is a symbolic link.
    Symlink,
    /// The path is not a directory.
    NotADirectory,
    /// Another user owns the directory.
    Owner {
        /// The directory's owner.
        owner: u32,
        /// The effective user ID.
        expected: u32,
    },
    /// The directory grants group or other permissions.
    Permissions {
        /// The directory's permission bits.
        mode: u32,
    },
}

impl fmt::Display for DirectoryProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Symlink => f.write_str("it is a symbolic link"),
            Self::NotADirectory => f.write_str("it is not a directory"),
            Self::Owner { owner, expected } => {
                write!(f, "user {owner} owns it, not user {expected}")
            }
            Self::Permissions { mode } => write!(
                f,
                "its mode {mode:04o} grants group or other permissions"
            ),
        }
    }
}

/// Why a socket path could not be resolved, bound, or connected to.
#[derive(Debug)]
#[non_exhaustive]
pub enum SocketError {
    /// The socket name does not match `[A-Za-z0-9][A-Za-z0-9._-]{0,63}`.
    InvalidName(String),
    /// `HUTERM_SOCKET_DIR` or an explicit socket path is not absolute, or
    /// an explicit path names no file.
    InvalidPath(PathBuf),
    /// The socket path does not fit `sun_path`.
    PathTooLong {
        /// The path that does not fit.
        path: PathBuf,
        /// The longest path that fits, in bytes.
        limit: usize,
    },
    /// The socket directory is not private to the effective user.
    UnsafeDirectory {
        /// The directory.
        path: PathBuf,
        /// What is wrong with it.
        problem: DirectoryProblem,
    },
    /// The peer runs as another user.
    ForeignPeer {
        /// The peer's user ID.
        peer: u32,
        /// The effective user ID.
        expected: u32,
    },
    /// The peer's credentials could not be read.
    PeerCredentials(io::Error),
    /// A file system or socket call on `path` failed.
    Io {
        /// What failed, as a verb phrase.
        action: &'static str,
        /// The path it failed on.
        path: PathBuf,
        /// The error.
        error: io::Error,
    },
}

impl fmt::Display for SocketError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName(name) => write!(
                f,
                "invalid socket name {name:?}: use 1 to \
                 {MAX_SOCKET_NAME_BYTES} letters, digits, '.', '_', or '-', \
                 starting with a letter or digit"
            ),
            Self::InvalidPath(path) => write!(
                f,
                "socket path {} must be absolute and name a file",
                path.display()
            ),
            Self::PathTooLong { path, limit } => write!(
                f,
                "socket path {} is {} bytes, longer than the {limit} a Unix \
                 socket address holds; use a shorter {SOCKET_DIR_VARIABLE} \
                 or socket name",
                path.display(),
                path.as_os_str().len()
            ),
            Self::UnsafeDirectory { path, problem } => write!(
                f,
                "refusing socket directory {}: {problem}",
                path.display()
            ),
            Self::ForeignPeer { peer, expected } => write!(
                f,
                "the socket's peer runs as user {peer}, not user {expected}"
            ),
            Self::PeerCredentials(error) => {
                write!(f, "cannot read the socket peer's credentials: {error}")
            }
            Self::Io {
                action,
                path,
                error,
            } => write!(f, "cannot {action} {}: {error}", path.display()),
        }
    }
}

impl std::error::Error for SocketError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::PeerCredentials(error) | Self::Io { error, .. } => {
                Some(error)
            }
            _ => None,
        }
    }
}

/// Whether `name` can name a socket: `[A-Za-z0-9][A-Za-z0-9._-]{0,63}`,
/// so it is also a safe path component.
#[must_use]
pub fn valid_socket_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= MAX_SOCKET_NAME_BYTES
        && bytes[0].is_ascii_alphanumeric()
        && bytes[1..]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(byte))
}

/// A socket's path and the directory whose permissions guard it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SocketPath {
    directory: PathBuf,
    path: PathBuf,
}

impl SocketPath {
    /// Resolves the socket named `name` from this process's environment
    /// and effective user.
    ///
    /// # Errors
    /// Refuses an invalid name, a relative `HUTERM_SOCKET_DIR`, and a path
    /// too long for a socket address.
    pub fn resolve(name: &str) -> Result<Self, SocketError> {
        resolve_in(
            name,
            |key| std::env::var_os(key),
            effective_uid(),
            |path| fs::metadata(path).ok().map(|metadata| metadata.uid()),
            cfg!(target_os = "linux"),
        )
    }

    /// The socket at `path`, whose parent directory must be private as a
    /// resolved one must.
    ///
    /// # Errors
    /// Refuses a relative path, one that names no file, and one too long
    /// for a socket address.
    pub fn explicit(path: impl Into<PathBuf>) -> Result<Self, SocketError> {
        let path = path.into();
        let directory = match (path.parent(), path.file_name()) {
            (Some(parent), Some(_)) if path.is_absolute() => {
                parent.to_path_buf()
            }
            _ => return Err(SocketError::InvalidPath(path)),
        };
        Self::checked(directory, path)
    }

    fn checked(directory: PathBuf, path: PathBuf) -> Result<Self, SocketError> {
        if path.as_os_str().len() > MAX_SOCKET_PATH_BYTES {
            return Err(SocketError::PathTooLong {
                path,
                limit: MAX_SOCKET_PATH_BYTES,
            });
        }
        Ok(Self { directory, path })
    }

    /// The socket's path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The directory that holds the socket.
    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Checks that the socket's directory is private to the effective
    /// user, without following a symbolic link.
    ///
    /// # Errors
    /// Returns what is wrong with the directory, or why it could not be
    /// inspected.
    pub fn check_directory(&self) -> Result<(), SocketError> {
        check_private_directory(&self.directory, effective_uid())
    }
}

fn resolve_in(
    name: &str,
    environment: impl Fn(&str) -> Option<OsString>,
    uid: u32,
    owner: impl Fn(&Path) -> Option<u32>,
    xdg_runtime: bool,
) -> Result<SocketPath, SocketError> {
    if !valid_socket_name(name) {
        return Err(SocketError::InvalidName(name.to_owned()));
    }
    let variable = |key: &str| {
        environment(key)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    let directory = if let Some(directory) = variable(SOCKET_DIR_VARIABLE) {
        if !directory.is_absolute() {
            return Err(SocketError::InvalidPath(directory));
        }
        directory
    } else if let Some(runtime) =
        variable("XDG_RUNTIME_DIR").filter(|runtime| {
            xdg_runtime && runtime.is_absolute() && owner(runtime) == Some(uid)
        })
    {
        runtime.join("huterm")
    } else {
        PathBuf::from(format!("/tmp/huterm-{uid}"))
    };
    let path = directory.join(name);
    SocketPath::checked(directory, path)
}

/// This process's effective user ID.
#[must_use]
pub fn effective_uid() -> u32 {
    nix::unistd::geteuid().as_raw()
}

fn check_private_directory(path: &Path, uid: u32) -> Result<(), SocketError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| SocketError::Io {
            action: "inspect socket directory",
            path: path.to_path_buf(),
            error,
        })?;
    let unsafe_directory = |problem| SocketError::UnsafeDirectory {
        path: path.to_path_buf(),
        problem,
    };
    let kind = metadata.file_type();
    if kind.is_symlink() {
        return Err(unsafe_directory(DirectoryProblem::Symlink));
    }
    if !kind.is_dir() {
        return Err(unsafe_directory(DirectoryProblem::NotADirectory));
    }
    if metadata.uid() != uid {
        return Err(unsafe_directory(DirectoryProblem::Owner {
            owner: metadata.uid(),
            expected: uid,
        }));
    }
    let mode = metadata.mode() & 0o7777;
    if mode & 0o077 != 0 {
        return Err(unsafe_directory(DirectoryProblem::Permissions { mode }));
    }
    Ok(())
}

/// The user ID of the process on the other end of `stream`, from
/// `SO_PEERCRED` on Linux and `getpeereid` elsewhere.
///
/// # Errors
/// Returns the system call's error.
pub fn peer_user(stream: &UnixStream) -> io::Result<u32> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        use nix::sys::socket::{getsockopt, sockopt::PeerCredentials};
        Ok(getsockopt(stream, PeerCredentials)?.uid())
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        let (uid, _) = nix::unistd::getpeereid(stream)?;
        Ok(uid.as_raw())
    }
}

/// Checks that the process on the other end of `stream` runs as user
/// `expected`.
///
/// # Errors
/// Returns [`SocketError::ForeignPeer`] for another user, or why the
/// credentials could not be read.
pub fn check_peer(
    stream: &UnixStream,
    expected: u32,
) -> Result<(), SocketError> {
    let peer = peer_user(stream).map_err(SocketError::PeerCredentials)?;
    if peer == expected {
        Ok(())
    } else {
        Err(SocketError::ForeignPeer { peer, expected })
    }
}

/// A socket file this process bound, which it removes when it stops
/// listening.
#[derive(Debug)]
pub struct SocketFile {
    path: PathBuf,
    device: u64,
    inode: u64,
}

impl SocketFile {
    /// The socket's path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Removes the socket file if it is still the one this process bound,
    /// so a socket another server bound at the same path since survives.
    /// Returns whether it removed the file.
    ///
    /// # Errors
    /// Returns the error of inspecting or removing the file, except that
    /// it is already gone.
    pub fn remove(self) -> io::Result<bool> {
        match fs::symlink_metadata(&self.path) {
            Ok(metadata)
                if metadata.dev() == self.device
                    && metadata.ino() == self.inode =>
            {
                fs::remove_file(&self.path)?;
                Ok(true)
            }
            Ok(_) => Ok(false),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }
}

/// Listens on `socket`: creates its directory with mode `0700` when it is
/// missing, checks that the directory is private, binds the socket, and
/// sets the socket file's mode to `0600`.
///
/// The mode is set after binding rather than under a restrictive umask,
/// because the umask is process-wide and would race with other threads
/// creating files; until it is set, the private directory keeps other
/// users out.
///
/// # Errors
/// Returns what is wrong with the directory, or the failing call's error.
/// An existing socket at the path fails to bind; detecting a stale one
/// belongs to the daemon.
pub fn bind(
    socket: &SocketPath,
) -> Result<(UnixListener, SocketFile), SocketError> {
    let io = |action, path: &Path| {
        let path = path.to_path_buf();
        move |error| SocketError::Io {
            action,
            path,
            error,
        }
    };
    match DirBuilder::new()
        .mode(DIRECTORY_MODE)
        .create(&socket.directory)
    {
        Err(error) if error.kind() != io::ErrorKind::AlreadyExists => {
            return Err(io("create socket directory", &socket.directory)(
                error,
            ));
        }
        _ => {}
    }
    socket.check_directory()?;
    let listener = UnixListener::bind(&socket.path)
        .map_err(io("bind socket", &socket.path))?;
    let finish = || -> Result<SocketFile, SocketError> {
        fs::set_permissions(&socket.path, Permissions::from_mode(SOCKET_MODE))
            .map_err(io("set the mode of socket", &socket.path))?;
        let metadata = fs::symlink_metadata(&socket.path)
            .map_err(io("inspect socket", &socket.path))?;
        Ok(SocketFile {
            path: socket.path.clone(),
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    };
    match finish() {
        Ok(file) => Ok((listener, file)),
        Err(error) => {
            let _ = fs::remove_file(&socket.path);
            Err(error)
        }
    }
}

/// Connects to `socket` after checking that its directory is private, and
/// checks that the server runs as the effective user.
///
/// # Errors
/// Returns what is wrong with the directory or the peer, or the failing
/// call's error, such as no server listening.
pub fn connect(socket: &SocketPath) -> Result<UnixStream, SocketError> {
    let uid = effective_uid();
    check_private_directory(&socket.directory, uid)?;
    let stream =
        UnixStream::connect(&socket.path).map_err(|error| SocketError::Io {
            action: "connect to socket",
            path: socket.path.clone(),
            error,
        })?;
    check_peer(&stream, uid)?;
    Ok(stream)
}

/// Private temporary socket directories for tests, so parallel tests never
/// share a socket.
#[cfg(any(test, feature = "test-support"))]
pub mod testing {
    use std::fs::{self, DirBuilder};
    use std::io;
    use std::os::unix::fs::DirBuilderExt;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::{DIRECTORY_MODE, SocketPath};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    /// A private directory under the system's temporary directory, removed
    /// with everything in it when dropped.
    #[derive(Debug)]
    pub struct TestDirectory {
        path: PathBuf,
    }

    impl TestDirectory {
        /// Creates a new private directory.
        ///
        /// # Panics
        /// Panics when the directory cannot be created.
        #[must_use]
        pub fn new() -> Self {
            let base = std::env::temp_dir();
            loop {
                let n = NEXT.fetch_add(1, Ordering::Relaxed);
                let path =
                    base.join(format!("huterm-{}-{n}", std::process::id()));
                match DirBuilder::new().mode(DIRECTORY_MODE).create(&path) {
                    Ok(()) => return Self { path },
                    Err(error)
                        if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => {
                        panic!("cannot create {}: {error}", path.display())
                    }
                }
            }
        }

        /// The directory.
        #[must_use]
        pub fn path(&self) -> &Path {
            &self.path
        }

        /// The socket `name` in this directory.
        ///
        /// # Panics
        /// Panics when the path is too long for a socket address.
        #[must_use]
        pub fn socket(&self, name: &str) -> SocketPath {
            SocketPath::explicit(self.path.join(name))
                .expect("a test socket path fits a socket address")
        }
    }

    impl Default for TestDirectory {
        fn default() -> Self {
            Self::new()
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

#[cfg(test)]
mod tests;
