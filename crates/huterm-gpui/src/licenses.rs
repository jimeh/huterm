//! The open source license notices this build ships, read from the
//! `licenses.json` artifact that `scripts/licenses.ts` generates. Packages
//! carry it beside the executable; unpackaged builds read the checkout's
//! copy. Loading is plain file work without GPUI, so the Licenses view runs
//! it off the UI thread.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;

const FILE_NAME: &str = "licenses.json";

/// The release targets whose components this executable contains. The
/// macOS package is universal, so a macOS build lists both architectures.
#[cfg(target_os = "macos")]
const CURRENT_TARGETS: &[&str] =
    &["aarch64-apple-darwin", "x86_64-apple-darwin"];
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const CURRENT_TARGETS: &[&str] = &["x86_64-unknown-linux-gnu"];
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
const CURRENT_TARGETS: &[&str] = &["aarch64-unknown-linux-gnu"];
#[cfg(not(any(
    target_os = "macos",
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    )
)))]
const CURRENT_TARGETS: &[&str] = &[];

#[derive(Deserialize)]
struct Artifact {
    schema: u32,
    components: Vec<RawComponent>,
    texts: HashMap<String, String>,
}

#[derive(Deserialize)]
struct RawComponent {
    id: String,
    name: String,
    version: Option<String>,
    kind: String,
    license: String,
    used: Option<String>,
    url: Option<String>,
    targets: Vec<String>,
    condition: Option<String>,
    texts: Vec<RawText>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawText {
    /// A text the artifact stores, by digest.
    Inline { title: String, text: String },
    /// A notice the Linux packager copies beside the artifact.
    Packaged { title: String, file: String },
}

/// One shipped component and the texts that carry its license.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Component {
    pub(crate) name: String,
    pub(crate) version: Option<String>,
    pub(crate) kind: String,
    /// The license as the component declares it.
    pub(crate) license: String,
    /// The alternative Huterm uses when the declared license offers a
    /// choice.
    pub(crate) used: Option<String>,
    pub(crate) url: Option<String>,
    pub(crate) texts: Vec<LicenseText>,
}

impl Component {
    /// Whether a lowercase `query` appears in the name, version, license,
    /// or kind. An empty query matches everything.
    pub(crate) fn matches(&self, query: &str) -> bool {
        query.is_empty()
            || [
                Some(self.name.as_str()),
                self.version.as_deref(),
                Some(self.license.as_str()),
                Some(self.kind.as_str()),
            ]
            .into_iter()
            .flatten()
            .any(|field| field.to_lowercase().contains(query))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LicenseText {
    pub(crate) title: String,
    pub(crate) body: Arc<str>,
}

/// The loaded notices for this build.
#[derive(Clone, Debug)]
pub(crate) struct Catalog {
    pub(crate) components: Vec<Component>,
}

/// What this build contains: the targets its executable covers and the
/// optional pieces it carries.
#[derive(Clone, Debug)]
pub(crate) struct Build {
    pub(crate) targets: &'static [&'static str],
    /// Whether Sparkle is linked, as in updater-enabled release packages.
    pub(crate) updater: bool,
    /// Whether this process runs from an `AppImage`.
    pub(crate) appimage: bool,
}

impl Build {
    pub(crate) fn current() -> Self {
        Self {
            targets: CURRENT_TARGETS,
            updater: cfg!(feature = "macos-updater"),
            appimage: cfg!(target_os = "linux") && launched_from_appimage(),
        }
    }

    fn includes(&self, component: &RawComponent) -> bool {
        let shipped = component
            .targets
            .iter()
            .any(|target| self.targets.contains(&target.as_str()));
        shipped
            && match component.condition.as_deref() {
                None => true,
                Some("macos-updater") => self.updater,
                Some("appimage") => self.appimage,
                Some(_) => false,
            }
    }
}

/// Whether this executable runs from an `AppImage` mount. The runtime sets
/// `APPDIR`, but terminals inherit it, so a Huterm started from a shell
/// inside an `AppImage` Huterm must also run from that directory.
fn launched_from_appimage() -> bool {
    let Some(appdir) = std::env::var_os("APPDIR") else {
        return false;
    };
    std::env::current_exe()
        .is_ok_and(|executable| runs_under(Path::new(&appdir), &executable))
}

/// Whether `executable`, which the OS reports with symlinks resolved, lies
/// under `appdir`, which may name a symlinked temporary directory.
fn runs_under(appdir: &Path, executable: &Path) -> bool {
    let appdir =
        std::fs::canonicalize(appdir).unwrap_or_else(|_| appdir.to_path_buf());
    executable.starts_with(appdir)
}

/// Why no notices could be shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LoadError {
    /// No artifact exists at any of these locations.
    Missing(Vec<PathBuf>),
    Unreadable {
        path: PathBuf,
        reason: String,
    },
    Invalid {
        path: PathBuf,
        reason: String,
    },
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(locations) => match locations.first() {
                Some(path) => write!(f, "Expected at {}", path.display()),
                None => write!(f, "No location to look in"),
            },
            Self::Unreadable { path, reason } => {
                write!(f, "Cannot read {}: {reason}", path.display())
            }
            Self::Invalid { path, reason } => {
                write!(f, "{} is invalid: {reason}", path.display())
            }
        }
    }
}

/// Loads the notices for the running build.
pub(crate) fn load() -> Result<Catalog, LoadError> {
    let executable = std::env::current_exe().ok();
    load_from(
        &locations(executable.as_deref(), cfg!(target_os = "macos")),
        &Build::current(),
    )
}

/// Where the artifact may be, in order: the macOS bundle's resources, the
/// Linux package's license directory, then, for an unpackaged build in
/// Cargo's `target/<profile>` or `target/<triple>/<profile>` layout, the
/// checkout's committed copy. The
/// checkout is found from the executable rather than compiled in, because
/// package checks reject an executable that contains its build path.
fn locations(executable: Option<&Path>, is_macos: bool) -> Vec<PathBuf> {
    let mut locations = Vec::with_capacity(3);
    let Some(directory) = executable.and_then(Path::parent) else {
        return locations;
    };
    if let Some(root) = directory.parent() {
        if is_macos {
            locations.push(root.join("Resources").join(FILE_NAME));
        }
        locations.push(root.join("share/licenses/huterm").join(FILE_NAME));
    }
    // Examples and test binaries sit one level below the profile directory.
    let profile = if matches!(
        directory.file_name().and_then(OsStr::to_str),
        Some("examples" | "deps")
    ) {
        directory.parent()
    } else {
        Some(directory)
    };
    // Only Cargo's own `target` directory, directly above the profile or
    // above a `--target` triple directory, so a packaged executable never
    // reads a stray file beside its package.
    let is_target = |dir: &&Path| dir.file_name() == Some(OsStr::new("target"));
    let above_profile = profile.and_then(Path::parent);
    let target = above_profile
        .filter(is_target)
        .or_else(|| above_profile.and_then(Path::parent).filter(is_target));
    if let Some(checkout) = target.and_then(Path::parent) {
        locations.push(checkout.join("third-party/licenses").join(FILE_NAME));
    }
    locations
}

fn load_from(
    locations: &[PathBuf],
    build: &Build,
) -> Result<Catalog, LoadError> {
    let path = locations
        .iter()
        .find(|path| path.is_file())
        .ok_or_else(|| LoadError::Missing(locations.to_vec()))?;
    let json = std::fs::read_to_string(path).map_err(|error| {
        LoadError::Unreadable {
            path: path.clone(),
            reason: error.to_string(),
        }
    })?;
    let directory = path.parent().unwrap_or_else(|| Path::new("."));
    let components = parse(&json, directory, build).map_err(|reason| {
        LoadError::Invalid {
            path: path.clone(),
            reason,
        }
    })?;
    Ok(Catalog { components })
}

/// The components `build` ships, in artifact order. A packaged notice
/// reads from `directory`; a component whose notice is absent, as in
/// unpackaged builds, which bundle none of those libraries, is left out.
fn parse(
    json: &str,
    directory: &Path,
    build: &Build,
) -> Result<Vec<Component>, String> {
    let artifact: Artifact =
        serde_json::from_str(json).map_err(|error| error.to_string())?;
    if artifact.schema != 1 {
        return Err(format!("unsupported schema {}", artifact.schema));
    }
    let shared: HashMap<String, Arc<str>> = artifact
        .texts
        .into_iter()
        .map(|(key, text)| (key, Arc::from(text)))
        .collect();
    let mut components = Vec::new();
    'components: for raw in artifact.components {
        if !build.includes(&raw) {
            continue;
        }
        let mut texts = Vec::with_capacity(raw.texts.len());
        for text in &raw.texts {
            match text {
                RawText::Inline { title, text } => {
                    let body = shared.get(text).cloned().ok_or_else(|| {
                        format!("{} refers to missing text {text}", raw.id)
                    })?;
                    texts.push(LicenseText {
                        title: title.clone(),
                        body,
                    });
                }
                RawText::Packaged { title, file } => {
                    if file.contains(['/', '\\']) || file.starts_with('.') {
                        return Err(format!(
                            "{} names an unsafe notice file {file}",
                            raw.id
                        ));
                    }
                    let Ok(body) =
                        std::fs::read_to_string(directory.join(file))
                    else {
                        continue 'components;
                    };
                    texts.push(LicenseText {
                        title: title.clone(),
                        body: Arc::from(body.trim_end()),
                    });
                }
            }
        }
        components.push(Component {
            name: raw.name,
            version: raw.version,
            kind: raw.kind,
            license: raw.license,
            used: raw.used,
            url: raw.url,
            texts,
        });
    }
    Ok(components)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINUX: Build = Build {
        targets: &["x86_64-unknown-linux-gnu"],
        updater: false,
        appimage: false,
    };

    fn artifact(components: &str) -> String {
        format!(
            r#"{{"schema": 1, "components": [{components}], "texts": {{"t1": "MIT text"}}}}"#
        )
    }

    fn component(id: &str, targets: &str, extra: &str) -> String {
        format!(
            r#"{{"id": "{id}", "name": "{id}", "kind": "Rust crate", "license": "MIT", "targets": [{targets}], {extra} "texts": [{{"title": "LICENSE", "text": "t1"}}]}}"#
        )
    }

    fn names(components: &[Component]) -> Vec<&str> {
        components.iter().map(|item| item.name.as_str()).collect()
    }

    /// The committed artifact in this checkout.
    const SOURCE_TREE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../third-party/licenses/licenses.json"
    );

    #[test]
    fn packaged_locations_come_before_the_checkout_copy() {
        let mac = locations(
            Some(Path::new("/Applications/Huterm.app/Contents/MacOS/huterm")),
            true,
        );
        assert_eq!(
            mac[..2],
            [
                PathBuf::from(
                    "/Applications/Huterm.app/Contents/Resources/licenses.json"
                ),
                PathBuf::from(
                    "/Applications/Huterm.app/Contents/share/licenses/huterm/licenses.json"
                ),
            ]
        );
        assert_eq!(mac.len(), 2, "{mac:?}");
        let linux = locations(Some(Path::new("/opt/huterm/bin/huterm")), false);
        assert_eq!(
            linux,
            [PathBuf::from(
                "/opt/huterm/share/licenses/huterm/licenses.json"
            )]
        );
        assert_eq!(locations(None, false), Vec::<PathBuf>::new());
    }

    #[test]
    fn unpackaged_builds_find_the_checkout_copy_from_the_target_directory() {
        let checkout =
            PathBuf::from("/src/huterm/third-party/licenses/licenses.json");
        for executable in [
            "/src/huterm/target/debug/huterm",
            "/src/huterm/target/release/huterm",
            "/src/huterm/target/debug/examples/palette_smoke",
            "/src/huterm/target/debug/deps/huterm_gpui-0123456789abcdef",
            "/src/huterm/target/aarch64-apple-darwin/release/huterm",
        ] {
            let found = locations(Some(Path::new(executable)), false);
            assert_eq!(found.last(), Some(&checkout), "{executable}");
        }
    }

    #[test]
    fn an_appimage_launch_is_recognized_through_a_symlinked_appdir() {
        let root = std::env::temp_dir()
            .join(format!("huterm-appdir-{}", std::process::id()));
        let real = root.join("real");
        std::fs::create_dir_all(real.join("usr/bin")).unwrap();
        let link = root.join("link");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let executable =
            std::fs::canonicalize(&real).unwrap().join("usr/bin/huterm");
        let inside = runs_under(&link, &executable);
        let elsewhere = runs_under(&link, Path::new("/opt/huterm/bin/huterm"));
        std::fs::remove_dir_all(&root).unwrap();
        assert!(inside);
        assert!(!elsewhere);
    }

    #[test]
    fn a_build_lists_only_the_components_its_targets_and_options_ship() {
        let json = artifact(
            &[
                component(
                    "everywhere",
                    r#""x86_64-unknown-linux-gnu", "aarch64-apple-darwin""#,
                    "",
                ),
                component("mac-only", r#""aarch64-apple-darwin""#, ""),
                component(
                    "sparkle",
                    r#""aarch64-apple-darwin""#,
                    r#""condition": "macos-updater","#,
                ),
                component(
                    "runtime",
                    r#""x86_64-unknown-linux-gnu""#,
                    r#""condition": "appimage","#,
                ),
                component(
                    "future",
                    r#""x86_64-unknown-linux-gnu""#,
                    r#""condition": "unknown","#,
                ),
            ]
            .join(","),
        );
        let directory = Path::new("/nonexistent");
        assert_eq!(
            names(&parse(&json, directory, &LINUX).unwrap()),
            ["everywhere"]
        );
        let appimage = Build {
            appimage: true,
            ..LINUX
        };
        assert_eq!(
            names(&parse(&json, directory, &appimage).unwrap()),
            ["everywhere", "runtime"]
        );
        let mac = Build {
            targets: &["aarch64-apple-darwin", "x86_64-apple-darwin"],
            updater: true,
            appimage: false,
        };
        assert_eq!(
            names(&parse(&json, directory, &mac).unwrap()),
            ["everywhere", "mac-only", "sparkle"]
        );
    }

    #[test]
    fn packaged_notices_read_beside_the_artifact_and_hide_their_component_when_absent()
     {
        let directory = std::env::temp_dir()
            .join(format!("huterm-licenses-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("present.copyright"), "Copyright X\n\n")
            .unwrap();
        let packaged = |id: &str, file: &str| {
            format!(
                r#"{{"id": "{id}", "name": "{id}", "kind": "Library", "license": "MIT", "targets": ["x86_64-unknown-linux-gnu"], "texts": [{{"title": "{file}", "file": "{file}"}}]}}"#
            )
        };
        let json = artifact(&format!(
            "{},{}",
            packaged("present", "present.copyright"),
            packaged("absent", "absent.copyright")
        ));
        let components = parse(&json, &directory, &LINUX).unwrap();
        std::fs::remove_dir_all(&directory).unwrap();
        assert_eq!(names(&components), ["present"]);
        assert_eq!(&*components[0].texts[0].body, "Copyright X");
        let unsafe_file = artifact(&packaged("escape", "../secret"));
        assert_eq!(
            parse(&unsafe_file, &directory, &LINUX),
            Err("escape names an unsafe notice file ../secret".to_owned())
        );
    }

    #[test]
    fn malformed_artifacts_and_missing_files_fail_with_a_reason() {
        let directory = Path::new("/nonexistent");
        assert_eq!(
            parse(
                r#"{"schema": 2, "components": [], "texts": {}}"#,
                directory,
                &LINUX
            ),
            Err("unsupported schema 2".to_owned())
        );
        let dangling = r#"{"schema": 1, "components": [{"id": "a", "name": "a", "kind": "Rust crate", "license": "MIT", "targets": ["x86_64-unknown-linux-gnu"], "texts": [{"title": "LICENSE", "text": "gone"}]}], "texts": {}}"#;
        assert_eq!(
            parse(dangling, directory, &LINUX),
            Err("a refers to missing text gone".to_owned())
        );
        assert!(parse("{", directory, &LINUX).is_err());
        let missing = vec![PathBuf::from("/nonexistent/licenses.json")];
        let error = load_from(&missing, &LINUX).unwrap_err();
        assert_eq!(error, LoadError::Missing(missing));
        assert_eq!(error.to_string(), "Expected at /nonexistent/licenses.json");
    }

    #[test]
    fn search_matches_name_version_license_and_kind_case_insensitively() {
        let component = Component {
            name: "Ghostty".to_owned(),
            version: Some("83fb956".to_owned()),
            kind: "Library".to_owned(),
            license: "MIT".to_owned(),
            used: None,
            url: None,
            texts: Vec::new(),
        };
        for query in ["", "ghost", "83fb", "mit", "library"] {
            assert!(component.matches(query), "{query}");
        }
        assert!(!component.matches("apache"));
    }

    #[test]
    fn the_committed_artifact_lists_this_builds_core_components() {
        let catalog =
            load_from(&[PathBuf::from(SOURCE_TREE)], &Build::current())
                .unwrap();
        let listed = names(&catalog.components);
        for name in [
            "Ghostty",
            "Rust standard library",
            "Zig standard library",
            "gpui-pre",
        ] {
            assert!(listed.contains(&name), "{name} missing from {listed:?}");
        }
        assert!(
            catalog.components.iter().all(|component| !component
                .texts
                .is_empty()
                && component.texts.iter().all(|text| !text.body.is_empty()))
        );
        let other_platform = if cfg!(target_os = "macos") {
            "x11rb"
        } else {
            "objc2"
        };
        assert!(!listed.contains(&other_platform));
    }
}
