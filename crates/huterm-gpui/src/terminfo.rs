use std::ffi::OsString;
use std::path::{Path, PathBuf};

use huterm_config::TerminalIdentity;

const HUTERM_TERM: &str = "xterm-huterm";
const COMPATIBILITY_TERM: &str = "xterm-256color";

pub(crate) fn environment(identity: TerminalIdentity) -> Vec<(String, String)> {
    let executable = std::env::current_exe().ok();
    environment_from(
        identity,
        private_directories(executable.as_deref(), cfg!(target_os = "macos")),
        |name| std::env::var_os(name),
    )
}

fn environment_from(
    identity: TerminalIdentity,
    private_directories: Vec<PathBuf>,
    get_environment: impl Fn(&str) -> Option<OsString>,
) -> Vec<(String, String)> {
    if identity == TerminalIdentity::Xterm256Color {
        return vec![("TERM".into(), COMPATIBILITY_TERM.into())];
    }

    let existing = search_directories(&get_environment)
        .into_iter()
        .any(|directory| entry_exists(&directory));
    let private_directory = private_directories
        .into_iter()
        .find(|directory| entry_exists(directory));
    let available = existing || private_directory.is_some();
    if identity == TerminalIdentity::Auto && !available {
        return vec![("TERM".into(), COMPATIBILITY_TERM.into())];
    }

    let mut result = vec![("TERM".into(), HUTERM_TERM.into())];
    if let Some(directory) = private_directory {
        let inherited = get_environment("TERMINFO_DIRS")
            .and_then(|value| value.into_string().ok());
        let directory = directory.to_string_lossy();
        let value = inherited.map_or_else(
            || format!("{directory}:"),
            |value| format!("{value}:{directory}"),
        );
        result.push(("TERMINFO_DIRS".into(), value));
    }
    result
}

fn private_directories(
    executable: Option<&Path>,
    is_macos: bool,
) -> Vec<PathBuf> {
    let mut directories = Vec::new();
    if let Some(executable) = executable
        && let Some(binary_directory) = executable.parent()
    {
        if is_macos && let Some(contents) = binary_directory.parent() {
            directories.push(contents.join("Resources/terminfo"));
        }
        if let Some(bundle) = binary_directory.parent() {
            directories.push(bundle.join("share/huterm/terminfo"));
        }
        let profile_directory = if binary_directory.file_name()
            == Some(std::ffi::OsStr::new("examples"))
        {
            binary_directory.parent()
        } else {
            Some(binary_directory)
        };
        if let Some(target_directory) = profile_directory.and_then(Path::parent)
        {
            directories.push(target_directory.join("terminfo"));
        }
    }
    directories
}

fn search_directories(
    get_environment: &impl Fn(&str) -> Option<OsString>,
) -> Vec<PathBuf> {
    let mut directories = Vec::new();
    if let Some(directory) = get_environment("TERMINFO") {
        let encoded = directory.to_string_lossy();
        if !encoded.starts_with("hex:") && !encoded.starts_with("b64:") {
            directories.push(PathBuf::from(directory));
        }
    }
    if let Some(home) = get_environment("HOME") {
        directories.push(PathBuf::from(home).join(".terminfo"));
    }
    match get_environment("TERMINFO_DIRS") {
        Some(value) => {
            for component in value.to_string_lossy().split(':') {
                if component.is_empty() {
                    directories.extend(default_directories());
                } else {
                    directories.push(PathBuf::from(component));
                }
            }
        }
        None => directories.extend(default_directories()),
    }
    directories
}

fn default_directories() -> [PathBuf; 3] {
    [
        PathBuf::from("/etc/terminfo"),
        PathBuf::from("/lib/terminfo"),
        PathBuf::from("/usr/share/terminfo"),
    ]
}

fn entry_exists(directory: &Path) -> bool {
    directory.join("x/xterm-huterm").is_file()
        || directory.join("78/xterm-huterm").is_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let directory = std::env::temp_dir().join(format!(
                "huterm-terminfo-{}-{nonce}",
                std::process::id()
            ));
            fs::create_dir_all(&directory).unwrap();
            Self(directory)
        }

        fn add_entry(&self) {
            fs::create_dir_all(self.0.join("x")).unwrap();
            fs::write(self.0.join("x/xterm-huterm"), b"fixture").unwrap();
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn resolved(
        identity: TerminalIdentity,
        private: Vec<PathBuf>,
        values: &[(&str, OsString)],
    ) -> Vec<(String, String)> {
        let values = values.iter().cloned().collect::<BTreeMap<_, _>>();
        environment_from(identity, private, |name| values.get(name).cloned())
    }

    #[test]
    fn auto_uses_private_entry_and_appends_it_to_inherited_search_path() {
        let private = TestDirectory::new();
        private.add_entry();
        assert_eq!(
            resolved(
                TerminalIdentity::Auto,
                vec![private.0.clone()],
                &[("TERMINFO_DIRS", OsString::from("/custom/one:"))],
            ),
            [
                ("TERM".into(), "xterm-huterm".into()),
                (
                    "TERMINFO_DIRS".into(),
                    format!("/custom/one::{}", private.0.display()),
                ),
            ]
        );
    }

    #[test]
    fn auto_falls_back_when_no_entry_is_available() {
        let missing = TestDirectory::new().0.join("missing");
        assert_eq!(
            resolved(
                TerminalIdentity::Auto,
                vec![missing],
                &[("TERMINFO_DIRS", OsString::from("/missing"))],
            ),
            [("TERM".into(), "xterm-256color".into())]
        );
    }

    #[test]
    fn auto_honors_an_entry_in_the_existing_search_path() {
        let existing = TestDirectory::new();
        existing.add_entry();
        let missing = existing.0.join("missing");
        assert_eq!(
            resolved(
                TerminalIdentity::Auto,
                vec![missing],
                &[("TERMINFO", existing.0.clone().into_os_string())],
            ),
            [("TERM".into(), "xterm-huterm".into())]
        );
    }

    #[test]
    fn encoded_terminfo_values_are_preserved_without_path_interpretation() {
        let private = TestDirectory::new();
        private.add_entry();
        assert_eq!(
            resolved(
                TerminalIdentity::Auto,
                vec![private.0.clone()],
                &[
                    ("TERMINFO", OsString::from("b64:encoded-database")),
                    ("TERMINFO_DIRS", OsString::from("/inherited")),
                ],
            ),
            [
                ("TERM".into(), "xterm-huterm".into()),
                (
                    "TERMINFO_DIRS".into(),
                    format!("/inherited:{}", private.0.display()),
                ),
            ]
        );
    }

    #[test]
    fn explicit_identities_force_the_requested_term() {
        let missing = TestDirectory::new().0.join("missing");
        assert_eq!(
            resolved(TerminalIdentity::XtermHuterm, vec![missing.clone()], &[],),
            [("TERM".into(), "xterm-huterm".into())]
        );
        assert_eq!(
            resolved(TerminalIdentity::Xterm256Color, vec![missing], &[]),
            [("TERM".into(), "xterm-256color".into())]
        );
    }

    #[test]
    fn linux_bundle_discovery_is_relative_to_the_executable() {
        let bundle = TestDirectory::new();
        let private = bundle.0.join("share/huterm/terminfo");
        fs::create_dir_all(private.join("x")).unwrap();
        fs::write(private.join("x/xterm-huterm"), b"fixture").unwrap();
        let executable = bundle.0.join("bin/huterm");
        let values = BTreeMap::from([(
            "TERMINFO_DIRS",
            OsString::from("/inherited/terminfo"),
        )]);
        assert_eq!(
            environment_from(
                TerminalIdentity::Auto,
                private_directories(Some(&executable), false),
                |name| values.get(name).cloned(),
            ),
            [
                ("TERM".into(), "xterm-huterm".into()),
                (
                    "TERMINFO_DIRS".into(),
                    format!("/inherited/terminfo:{}", private.display()),
                ),
            ]
        );
    }

    #[test]
    fn macos_bundle_discovery_is_relative_to_the_executable() {
        let bundle = TestDirectory::new();
        let private = bundle.0.join("Contents/Resources/terminfo");
        fs::create_dir_all(private.join("x")).unwrap();
        fs::write(private.join("x/xterm-huterm"), b"fixture").unwrap();
        let executable = bundle.0.join("Contents/MacOS/Huterm");
        assert_eq!(
            environment_from(
                TerminalIdentity::Auto,
                private_directories(Some(&executable), true),
                |_| Some(OsString::from("/missing")),
            ),
            [
                ("TERM".into(), "xterm-huterm".into()),
                (
                    "TERMINFO_DIRS".into(),
                    format!("/missing:{}", private.display()),
                ),
            ]
        );
    }

    #[test]
    fn development_discovery_is_executable_relative_for_binary_and_example() {
        let development = TestDirectory::new();
        let private = development.0.join("target/terminfo");
        fs::create_dir_all(private.join("x")).unwrap();
        fs::write(private.join("x/xterm-huterm"), b"fixture").unwrap();
        for executable in [
            development.0.join("target/debug/huterm"),
            development.0.join("target/release/huterm"),
            development.0.join("target/debug/examples/query-smoke"),
        ] {
            assert_eq!(
                environment_from(
                    TerminalIdentity::Auto,
                    private_directories(Some(&executable), false),
                    |_| Some(OsString::from("/missing")),
                ),
                [
                    ("TERM".into(), "xterm-huterm".into()),
                    (
                        "TERMINFO_DIRS".into(),
                        format!("/missing:{}", private.display()),
                    ),
                ],
                "executable={}",
                executable.display()
            );
        }
    }
}
