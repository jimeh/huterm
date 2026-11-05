//! Crash-safe file replacement within one directory.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

/// The suffix of a write in progress.
const TEMPORARY_SUFFIX: &str = ".tmp";

/// Replaces `directory/name` with `bytes`: writes `name.tmp` (`0600`),
/// syncs it, runs `before_rename`, renames it over the target, then syncs
/// the directory. An interrupted write leaves the previous file in place;
/// `before_rename` lets tests interrupt one.
pub(crate) fn write_with(
    directory: &Path,
    name: &str,
    bytes: &[u8],
    before_rename: impl FnOnce(&Path) -> io::Result<()>,
) -> io::Result<()> {
    let temporary = directory.join(format!("{name}{TEMPORARY_SUFFIX}"));
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    before_rename(&temporary)?;
    fs::rename(&temporary, directory.join(name))?;
    sync_directory(directory)
}

/// Renames `from` to `to` within `directory`, replacing `to`, and syncs
/// the directory.
pub(crate) fn rename(directory: &Path, from: &str, to: &str) -> io::Result<()> {
    fs::rename(directory.join(from), directory.join(to))?;
    sync_directory(directory)
}

/// Syncs a directory, so renames and creations in it survive a crash.
pub(crate) fn sync_directory(directory: &Path) -> io::Result<()> {
    File::open(directory)?.sync_all()
}

/// Removes every leftover temporary file in `directory`, which only an
/// interrupted write leaves. Only the lock holder may call this.
pub(crate) fn remove_temporaries(directory: &Path) -> io::Result<Vec<PathBuf>> {
    let mut removed = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let is_temporary = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.ends_with(TEMPORARY_SUFFIX));
        if is_temporary && entry.file_type()?.is_file() {
            fs::remove_file(entry.path())?;
            removed.push(entry.path());
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::tests::TempDir;
    use std::os::unix::fs::PermissionsExt;

    fn write(directory: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
        write_with(directory, name, bytes, |_| Ok(()))
    }

    #[test]
    fn a_write_replaces_the_file_privately_and_leaves_no_temporary() {
        let dir = TempDir::new();
        write(dir.path(), "state.json", b"one").unwrap();
        write(dir.path(), "state.json", b"two").unwrap();
        let target = dir.path().join("state.json");
        assert_eq!(fs::read(&target).unwrap(), b"two");
        let mode = fs::metadata(&target).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        assert!(!dir.path().join("state.json.tmp").exists());
    }

    #[test]
    fn an_interrupted_write_keeps_the_old_file_and_cleanup_removes_its_temporary()
     {
        let dir = TempDir::new();
        write(dir.path(), "state.json", b"old").unwrap();
        let error = write_with(dir.path(), "state.json", b"new", |temporary| {
            assert_eq!(fs::read(temporary).unwrap(), b"new");
            Err(io::Error::other("interrupted"))
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "interrupted");
        assert_eq!(fs::read(dir.path().join("state.json")).unwrap(), b"old");
        fs::write(dir.path().join("keep.json"), b"kept").unwrap();
        let removed = remove_temporaries(dir.path()).unwrap();
        assert_eq!(removed, [dir.path().join("state.json.tmp")]);
        assert!(dir.path().join("keep.json").exists());
        assert_eq!(fs::read(dir.path().join("state.json")).unwrap(), b"old");
    }
}
