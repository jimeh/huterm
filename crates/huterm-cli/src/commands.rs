//! One module per subcommand, and what they share.
use std::ffi::OsStr;
use std::hash::BuildHasher as _;
use std::os::unix::fs::MetadataExt as _;
use std::os::unix::process::CommandExt as _;
use std::path::Path;
use std::process::Command;

pub(crate) mod ssh;
pub(crate) mod ssh_cache;
pub(crate) mod sudo;

/// Huterm's own terminfo entry name.
pub(crate) const HUTERM_TERM: &str = "xterm-huterm";

/// The widely installed entry the wrappers fall back to.
pub(crate) const COMPATIBILITY_TERM: &str = "xterm-256color";

/// Whether this process's `TERM` is Huterm's own entry.
pub(crate) fn term_is_huterm() -> bool {
    std::env::var_os("TERM").as_deref() == Some(OsStr::new(HUTERM_TERM))
}

/// Replaces this process with `command` and returns only on failure, so
/// the program keeps the process ID, standard streams, signals, and exit
/// status.
pub(crate) fn exec(command: &mut Command) -> anyhow::Error {
    let error = command.exec();
    anyhow::Error::new(error).context(format!(
        "cannot run {}",
        command.get_program().to_string_lossy()
    ))
}

/// A random number from the standard library's per-process hash seeds,
/// which the operating system's random source provides.
pub(crate) fn random() -> u64 {
    std::hash::RandomState::new().hash_one(std::time::SystemTime::now())
}

/// What a file or directory must be before Huterm trusts it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Kind {
    Directory,
    File,
}

/// Checks, without following a symbolic link, that `path` is a `kind`
/// owned by user `uid`.
pub(crate) fn check_owned(
    path: &Path,
    kind: Kind,
    uid: u32,
) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        format!("cannot inspect {}: {error}", path.display())
    })?;
    let file_type = metadata.file_type();
    let problem = if file_type.is_symlink() {
        Some("it is a symbolic link".to_owned())
    } else if kind == Kind::Directory && !file_type.is_dir() {
        Some("it is not a directory".to_owned())
    } else if kind == Kind::File && !file_type.is_file() {
        Some("it is not a regular file".to_owned())
    } else if metadata.uid() != uid {
        Some(format!("user {} owns it, not user {uid}", metadata.uid()))
    } else {
        None
    };
    match problem {
        Some(problem) => Err(format!("refusing {}: {problem}", path.display())),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;

    #[test]
    fn ownership_checks_refuse_links_wrong_kinds_and_other_owners() {
        let root = std::env::temp_dir()
            .join(format!("huterm-cli-owned-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("directory")).unwrap();
        std::fs::write(root.join("file"), "").unwrap();
        symlink(root.join("directory"), root.join("link")).unwrap();
        let uid = huterm_socket::effective_uid();
        assert!(
            check_owned(&root.join("directory"), Kind::Directory, uid).is_ok()
        );
        assert!(check_owned(&root.join("file"), Kind::File, uid).is_ok());
        for (name, kind, owner, problem) in [
            ("link", Kind::Directory, uid, "symbolic link"),
            ("file", Kind::Directory, uid, "not a directory"),
            ("directory", Kind::File, uid, "not a regular file"),
            ("directory", Kind::Directory, uid + 1, "owns it"),
            ("file", Kind::File, uid + 1, "owns it"),
        ] {
            let error = check_owned(&root.join(name), kind, owner).unwrap_err();
            assert!(error.contains(problem), "{name}: {error}");
        }
        std::fs::remove_dir_all(&root).unwrap();
    }
}
