//! Where saved state lives.

use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

/// Why no state root could be resolved.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PathError {
    /// `HUTERM_STATE_DIR` is set to a relative path.
    RelativeOverride,
    /// Neither `HUTERM_STATE_DIR`, an absolute `XDG_STATE_HOME`, nor an
    /// absolute `HOME` is set.
    NoHome,
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::RelativeOverride => {
                "HUTERM_STATE_DIR is not an absolute path"
            }
            Self::NoHome => "no state directory: HOME is not set",
        })
    }
}

impl std::error::Error for PathError {}

/// Resolves the state root from `environment`: `HUTERM_STATE_DIR`, else
/// `$XDG_STATE_HOME/huterm`, else `$HOME/.local/state/huterm`, on every
/// platform. Empty values count as unset, and a relative `XDG_STATE_HOME`
/// is ignored as the XDG specification requires.
///
/// # Errors
/// Rejects a relative `HUTERM_STATE_DIR`, and reports when no absolute
/// home is available.
pub fn state_root(
    environment: impl Fn(&str) -> Option<OsString>,
) -> Result<PathBuf, PathError> {
    let variable = |name: &str| {
        environment(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    if let Some(root) = variable("HUTERM_STATE_DIR") {
        return if root.is_absolute() {
            Ok(root)
        } else {
            Err(PathError::RelativeOverride)
        };
    }
    if let Some(state) = variable("XDG_STATE_HOME").filter(|p| p.is_absolute())
    {
        return Ok(state.join("huterm"));
    }
    variable("HOME")
        .filter(|home| home.is_absolute())
        .map(|home| home.join(".local/state/huterm"))
        .ok_or(PathError::NoHome)
}

/// Whether `name` can name a socket scope: `[A-Za-z0-9][A-Za-z0-9._-]{0,63}`,
/// the IPC socket grammar, so it is also a safe path component.
#[must_use]
pub fn valid_socket_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 64
        && bytes[0].is_ascii_alphanumeric()
        && bytes[1..]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(byte))
}

/// The scope directory for a valid socket name under `root`.
pub(crate) fn scope_directory(root: &Path, socket_name: &str) -> PathBuf {
    root.join("sockets").join(socket_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(pairs: &[(&str, &str)]) -> Result<PathBuf, PathError> {
        state_root(|name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        })
    }

    #[test]
    fn the_override_wins_then_xdg_state_home_then_home() {
        let all = [
            ("HUTERM_STATE_DIR", "/override"),
            ("XDG_STATE_HOME", "/xdg"),
            ("HOME", "/home/user"),
        ];
        assert_eq!(resolve(&all), Ok(PathBuf::from("/override")));
        assert_eq!(resolve(&all[1..]), Ok(PathBuf::from("/xdg/huterm")));
        assert_eq!(
            resolve(&all[2..]),
            Ok(PathBuf::from("/home/user/.local/state/huterm"))
        );
    }

    #[test]
    fn empty_and_relative_values_never_resolve_to_the_working_directory() {
        // Empty counts as unset.
        assert_eq!(
            resolve(&[("HUTERM_STATE_DIR", ""), ("HOME", "/h")]),
            Ok(PathBuf::from("/h/.local/state/huterm"))
        );
        // A relative XDG_STATE_HOME is ignored; a relative override is an
        // error rather than a silent fallback to real state.
        assert_eq!(
            resolve(&[("XDG_STATE_HOME", "state"), ("HOME", "/h")]),
            Ok(PathBuf::from("/h/.local/state/huterm"))
        );
        assert_eq!(
            resolve(&[("HUTERM_STATE_DIR", "state"), ("HOME", "/h")]),
            Err(PathError::RelativeOverride)
        );
        assert_eq!(resolve(&[("HOME", "relative")]), Err(PathError::NoHome));
        assert_eq!(resolve(&[]), Err(PathError::NoHome));
    }

    #[test]
    fn socket_names_follow_the_ipc_grammar() {
        let longest = format!("a{}", "b".repeat(63));
        for name in ["default", "a", "A.b_c-9", longest.as_str()] {
            assert!(valid_socket_name(name), "{name}");
        }
        let too_long = format!("{longest}c");
        for name in [
            "",
            ".hidden",
            "-dash",
            "_under",
            "a/b",
            "..",
            "a b",
            "é",
            too_long.as_str(),
        ] {
            assert!(!valid_socket_name(name), "{name}");
        }
    }
}
