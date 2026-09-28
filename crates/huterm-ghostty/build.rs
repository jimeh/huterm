//! Builds the static libghostty-vt archive from the verified Ghostty source.
//!
//! Mise prepares and hash-checks the source tree, and Cargo configuration
//! points `GHOSTTY_SOURCE_DIR` at it. Zig writes mutable `zig-pkg`
//! directories beside `build.zig`, so every build runs from a fresh private
//! copy under `OUT_DIR` and never touches the verified tree. Zig's caches sit
//! beside that copy, or in Zig's global cache, so they survive a refresh.
//!
//! The staging and option parsing below are plain functions with unit tests;
//! `scripts/ghostty-build.test.ts` compiles this file with `rustc --test`.

use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

const SOURCE_VAR: &str = "GHOSTTY_SOURCE_DIR";
const OPTIMIZE_VAR: &str = "HUTERM_GHOSTTY_OPTIMIZE";
const CPU_VAR: &str = "HUTERM_GHOSTTY_CPU";
const ZIG_VAR: &str = "ZIG";
const DEPLOYMENT_VAR: &str = "MACOSX_DEPLOYMENT_TARGET";

/// The pin's build replaces any requested macOS minimum with this floor.
const GHOSTTY_MACOS_FLOOR: (u32, u32) = (13, 0);

/// Zig optimization modes accepted by Ghostty's `-Doptimize`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Optimize {
    Debug,
    ReleaseSafe,
    ReleaseFast,
    ReleaseSmall,
}

impl Optimize {
    fn parse(value: &str) -> Result<Self, BuildError> {
        match value {
            "Debug" => Ok(Self::Debug),
            "ReleaseSafe" => Ok(Self::ReleaseSafe),
            "ReleaseFast" => Ok(Self::ReleaseFast),
            "ReleaseSmall" => Ok(Self::ReleaseSmall),
            other => Err(BuildError::Option(format!(
                "{OPTIMIZE_VAR}={other:?} is not one of Debug, ReleaseSafe, \
                 ReleaseFast, or ReleaseSmall"
            ))),
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Debug => "Debug",
            Self::ReleaseSafe => "ReleaseSafe",
            Self::ReleaseFast => "ReleaseFast",
            Self::ReleaseSmall => "ReleaseSmall",
        }
    }
}

#[derive(Debug)]
enum BuildError {
    Option(String),
    Io(String, io::Error),
    Zig(String),
}

impl fmt::Display for BuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Option(message) | Self::Zig(message) => {
                formatter.write_str(message)
            }
            Self::Io(context, error) => write!(formatter, "{context}: {error}"),
        }
    }
}

/// Validated inputs for one native build.
#[derive(Debug, Eq, PartialEq)]
struct Options {
    optimize: Optimize,
    cpu: String,
    /// Zig `-Dtarget`, or `None` for the native host.
    zig_target: Option<String>,
}

impl Options {
    fn from_values(
        optimize: Option<&str>,
        cpu: Option<&str>,
        target: &str,
        host: &str,
        deployment: Option<&str>,
    ) -> Result<Self, BuildError> {
        let optimize = Optimize::parse(optimize.ok_or_else(|| {
            BuildError::Option(format!("{OPTIMIZE_VAR} must be set"))
        })?)?;
        let cpu = cpu.ok_or_else(|| {
            BuildError::Option(format!("{CPU_VAR} must be set"))
        })?;
        if cpu.is_empty()
            || !cpu.bytes().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(byte, b'_' | b'+' | b'-')
            })
        {
            return Err(BuildError::Option(format!(
                "{CPU_VAR}={cpu:?} is not a Zig CPU model"
            )));
        }
        Ok(Self {
            optimize,
            cpu: cpu.to_owned(),
            zig_target: zig_target(target, host, deployment)?,
        })
    }

    fn zig_arguments(
        &self,
        install: &Path,
        local_cache: &Path,
    ) -> Vec<OsString> {
        let mut arguments: Vec<OsString> = vec![
            "build".into(),
            "-Demit-lib-vt=true".into(),
            // Linux otherwise defaults to the GTK runtime, which lib-vt does
            // not use but records in its build options.
            "-Dapp-runtime=none".into(),
            // Huterm links only the host archive; skip Apple bundles.
            "-Demit-xcframework=false".into(),
            format!("-Doptimize={}", self.optimize.as_str()).into(),
            format!("-Dcpu={}", self.cpu).into(),
        ];
        if let Some(target) = &self.zig_target {
            arguments.push(format!("-Dtarget={target}").into());
        }
        arguments.push("--prefix".into());
        arguments.push(install.into());
        arguments.push("--cache-dir".into());
        arguments.push(local_cache.into());
        arguments
    }
}

/// Maps a Rust target triple to a Zig target. Linux host builds stay native
/// so Zig uses the host's glibc; macOS always names its minimum version.
fn zig_target(
    target: &str,
    host: &str,
    deployment: Option<&str>,
) -> Result<Option<String>, BuildError> {
    let arch = match target.split('-').next() {
        Some("aarch64") => "aarch64",
        Some("x86_64") => "x86_64",
        _ => {
            return Err(BuildError::Option(format!(
                "unsupported target {target}: huterm-ghostty supports \
                 aarch64 and x86_64"
            )));
        }
    };
    if target.ends_with("-apple-darwin") {
        let version = deployment.unwrap_or("14.0");
        let (major, minor) = parse_version(version).ok_or_else(|| {
            BuildError::Option(format!(
                "{DEPLOYMENT_VAR}={version:?} is not a macOS version"
            ))
        })?;
        if (major, minor) < GHOSTTY_MACOS_FLOOR {
            return Err(BuildError::Option(format!(
                "{DEPLOYMENT_VAR}={version} is older than the macOS \
                 {}.{} minimum that Ghostty's build applies",
                GHOSTTY_MACOS_FLOOR.0, GHOSTTY_MACOS_FLOOR.1
            )));
        }
        return Ok(Some(format!("{arch}-macos.{major}.{minor}")));
    }
    if target.ends_with("-unknown-linux-gnu") {
        return Ok((target != host).then(|| format!("{arch}-linux-gnu")));
    }
    Err(BuildError::Option(format!(
        "unsupported target {target}: huterm-ghostty supports macOS and \
         Linux GNU targets"
    )))
}

fn parse_version(version: &str) -> Option<(u32, u32)> {
    let mut parts = version.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = match parts.next() {
        Some(minor) => minor.parse().ok()?,
        None => 0,
    };
    if parts
        .next()
        .is_some_and(|patch| patch.parse::<u32>().is_err())
    {
        return None;
    }
    Some((major, minor))
}

/// Replaces `destination` with a copy of `source`, preserving symbolic
/// links. The source is only read.
fn stage_source(source: &Path, destination: &Path) -> Result<(), BuildError> {
    let metadata = fs::metadata(source).map_err(|error| {
        BuildError::Io(format!("reading {}", source.display()), error)
    })?;
    if !metadata.is_dir() {
        return Err(BuildError::Option(format!(
            "{SOURCE_VAR}={} is not a directory; run `mise run \
             ghostty:prepare`",
            source.display()
        )));
    }
    if !source.join("build.zig").is_file() {
        return Err(BuildError::Option(format!(
            "{} has no build.zig; run `mise run ghostty:prepare`",
            source.display()
        )));
    }
    match fs::remove_dir_all(destination) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(BuildError::Io(
                format!("removing {}", destination.display()),
                error,
            ));
        }
    }
    copy_tree(source, destination)
}

fn copy_tree(source: &Path, destination: &Path) -> Result<(), BuildError> {
    let io = |context: &str, path: &Path| {
        let context = format!("{context} {}", path.display());
        move |error| BuildError::Io(context, error)
    };
    fs::create_dir_all(destination).map_err(io("creating", destination))?;
    for entry in fs::read_dir(source).map_err(io("reading", source))? {
        let entry = entry.map_err(io("reading", source))?;
        let from = entry.path();
        let to = destination.join(entry.file_name());
        let kind = entry.file_type().map_err(io("inspecting", &from))?;
        if kind.is_symlink() {
            let link = fs::read_link(&from).map_err(io("reading", &from))?;
            symlink(&link, &to).map_err(io("linking", &to))?;
        } else if kind.is_dir() {
            copy_tree(&from, &to)?;
        } else if kind.is_file() {
            fs::copy(&from, &to).map_err(io("copying", &from))?;
        } else {
            return Err(BuildError::Option(format!(
                "unexpected source entry {}",
                from.display()
            )));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn symlink(link: &Path, to: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(link, to)
}

#[cfg(not(unix))]
fn symlink(_link: &Path, _to: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "huterm-ghostty builds only on Unix hosts",
    ))
}

/// Paths used by one build, all under `OUT_DIR`.
struct Layout {
    source: PathBuf,
    install: PathBuf,
    local_cache: PathBuf,
}

impl Layout {
    fn new(out_dir: &Path) -> Self {
        Self {
            source: out_dir.join("ghostty-source"),
            install: out_dir.join("ghostty-install"),
            local_cache: out_dir.join("zig-local-cache"),
        }
    }
}

fn run_zig(
    zig: &OsString,
    layout: &Layout,
    options: &Options,
) -> Result<(), BuildError> {
    let mut command = Command::new(zig);
    command
        .current_dir(&layout.source)
        .args(options.zig_arguments(&layout.install, &layout.local_cache));
    // Commit hooks export repository paths that would point Ghostty's version
    // probe at Huterm; the ceiling stops discovery above the private copy.
    for (key, _) in std::env::vars_os() {
        if key.to_str().is_some_and(|key| key.starts_with("GIT_")) {
            command.env_remove(key);
        }
    }
    if let Some(parent) = layout.source.parent() {
        command.env("GIT_CEILING_DIRECTORIES", parent);
    }
    let status = command.status().map_err(|error| {
        BuildError::Io(
            format!(
                "running {}; install the pinned Zig with `mise install`",
                zig.to_string_lossy()
            ),
            error,
        )
    })?;
    if !status.success() {
        return Err(BuildError::Zig(format!(
            "zig build failed with {status} in {}",
            layout.source.display()
        )));
    }
    Ok(())
}

fn required_path(name: &str) -> Result<PathBuf, BuildError> {
    std::env::var_os(name)
        .map(PathBuf::from)
        .ok_or_else(|| BuildError::Option(format!("{name} must be set")))
}

fn optional(name: &str) -> Option<String> {
    println!("cargo:rerun-if-env-changed={name}");
    std::env::var(name).ok()
}

fn build() -> Result<(), BuildError> {
    let manifest_dir = required_path("CARGO_MANIFEST_DIR")?;
    // The manifest pins the source revision and tree hash that
    // `ghostty:prepare` verified, so it stands in for every source file.
    let manifest = manifest_dir.join("../../scripts/ghostty-source.json");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={}", manifest.display());
    println!("cargo:rerun-if-env-changed={SOURCE_VAR}");

    let source = required_path(SOURCE_VAR).map_err(|_| {
        BuildError::Option(format!(
            "{SOURCE_VAR} must be set; build through Cargo in the Huterm \
             workspace after `mise run ghostty:prepare`"
        ))
    })?;
    let target = std::env::var("TARGET").unwrap_or_default();
    let host = std::env::var("HOST").unwrap_or_default();
    let options = Options::from_values(
        optional(OPTIMIZE_VAR).as_deref(),
        optional(CPU_VAR).as_deref(),
        &target,
        &host,
        optional(DEPLOYMENT_VAR).as_deref(),
    )?;
    let zig =
        optional(ZIG_VAR).map_or_else(|| OsString::from("zig"), OsString::from);

    let out_dir = required_path("OUT_DIR")?;
    let layout = Layout::new(&out_dir);
    stage_source(&source, &layout.source)?;
    run_zig(&zig, &layout, &options)?;

    let library = layout.install.join("lib");
    if !library.join("libghostty-vt.a").is_file() {
        return Err(BuildError::Zig(format!(
            "zig build did not produce {}",
            library.join("libghostty-vt.a").display()
        )));
    }
    // The copy exists only for Zig; a rebuild stages a fresh one. Dropping
    // it keeps each build directory about 650 MB smaller.
    if let Err(error) = fs::remove_dir_all(&layout.source) {
        println!(
            "cargo:warning=could not remove {}: {error}",
            layout.source.display()
        );
    }
    println!("cargo:rustc-link-search=native={}", library.display());
    println!("cargo:rustc-link-lib=static=ghostty-vt");
    // The archive needs only libc, which Rust's standard library already
    // links on both platforms.
    println!(
        "cargo:rustc-env=HUTERM_GHOSTTY_BUILT_OPTIMIZE={}",
        options.optimize.as_str()
    );
    println!("cargo:include={}", layout.install.join("include").display());
    Ok(())
}

fn main() {
    if let Err(error) = build() {
        eprintln!("huterm-ghostty build failed: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(
        optimize: Option<&str>,
        cpu: Option<&str>,
        target: &str,
        deployment: Option<&str>,
    ) -> Result<Options, BuildError> {
        Options::from_values(
            optimize,
            cpu,
            target,
            "aarch64-apple-darwin",
            deployment,
        )
    }

    #[test]
    fn optimize_modes_are_exact_zig_names() {
        for name in ["Debug", "ReleaseSafe", "ReleaseFast", "ReleaseSmall"] {
            assert_eq!(Optimize::parse(name).unwrap().as_str(), name);
        }
        for invalid in ["", "release", "releasefast", "Release", "Fast"] {
            assert!(Optimize::parse(invalid).is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn options_require_both_build_variables() {
        let target = "aarch64-apple-darwin";
        assert!(options(None, Some("baseline"), target, None).is_err());
        assert!(options(Some("ReleaseFast"), None, target, None).is_err());
        for cpu in ["", "base line", "x;y", "native\n"] {
            assert!(
                options(Some("ReleaseFast"), Some(cpu), target, None).is_err(),
                "{cpu:?}"
            );
        }
        let parsed =
            options(Some("ReleaseSafe"), Some("x86_64_v2+avx"), target, None)
                .unwrap();
        assert_eq!(parsed.optimize, Optimize::ReleaseSafe);
        assert_eq!(parsed.cpu, "x86_64_v2+avx");
    }

    #[test]
    fn targets_map_to_zig_and_keep_linux_host_builds_native() {
        let target = |target, host, deployment| {
            zig_target(target, host, deployment)
                .map_err(|error| error.to_string())
        };
        assert_eq!(
            target(
                "aarch64-apple-darwin",
                "aarch64-apple-darwin",
                Some("14.0")
            ),
            Ok(Some("aarch64-macos.14.0".to_owned()))
        );
        assert_eq!(
            target("x86_64-apple-darwin", "aarch64-apple-darwin", Some("15")),
            Ok(Some("x86_64-macos.15.0".to_owned()))
        );
        assert_eq!(
            target("aarch64-apple-darwin", "aarch64-apple-darwin", None),
            Ok(Some("aarch64-macos.14.0".to_owned()))
        );
        assert_eq!(
            target(
                "x86_64-unknown-linux-gnu",
                "x86_64-unknown-linux-gnu",
                None
            ),
            Ok(None)
        );
        assert_eq!(
            target(
                "aarch64-unknown-linux-gnu",
                "x86_64-unknown-linux-gnu",
                None
            ),
            Ok(Some("aarch64-linux-gnu".to_owned()))
        );
        for (unsupported, deployment) in [
            ("x86_64-pc-windows-msvc", None),
            ("wasm32-unknown-unknown", None),
            ("aarch64-apple-ios", None),
            ("x86_64-unknown-linux-musl", None),
            ("aarch64-apple-darwin", Some("12.7")),
            ("aarch64-apple-darwin", Some("fourteen")),
            ("aarch64-apple-darwin", Some("14.0.x")),
        ] {
            assert!(
                target(unsupported, "aarch64-apple-darwin", deployment)
                    .is_err(),
                "{unsupported} {deployment:?}"
            );
        }
    }

    #[test]
    fn zig_arguments_name_every_option_and_private_path() {
        let parsed = options(
            Some("ReleaseFast"),
            Some("baseline"),
            "x86_64-apple-darwin",
            Some("14.0"),
        )
        .unwrap();
        let arguments = parsed
            .zig_arguments(Path::new("/out/install"), Path::new("/out/cache"));
        let arguments: Vec<_> = arguments
            .iter()
            .map(|value| value.to_str().unwrap())
            .collect();
        assert_eq!(
            arguments,
            [
                "build",
                "-Demit-lib-vt=true",
                "-Dapp-runtime=none",
                "-Demit-xcframework=false",
                "-Doptimize=ReleaseFast",
                "-Dcpu=baseline",
                "-Dtarget=x86_64-macos.14.0",
                "--prefix",
                "/out/install",
                "--cache-dir",
                "/out/cache",
            ]
        );
    }

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "huterm-ghostty-build-{name}-{}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn snapshot(root: &Path) -> Vec<(String, String)> {
        fn visit(
            root: &Path,
            directory: &Path,
            out: &mut Vec<(String, String)>,
        ) {
            let mut entries: Vec<_> = fs::read_dir(directory)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect();
            entries.sort();
            for path in entries {
                let name =
                    path.strip_prefix(root).unwrap().display().to_string();
                let metadata = fs::symlink_metadata(&path).unwrap();
                if metadata.file_type().is_symlink() {
                    let link = fs::read_link(&path).unwrap();
                    out.push((name, format!("link:{}", link.display())));
                } else if metadata.is_dir() {
                    out.push((name, "dir".to_owned()));
                    visit(root, &path, out);
                } else {
                    out.push((name, fs::read_to_string(&path).unwrap()));
                }
            }
        }
        let mut out = Vec::new();
        visit(root, root, &mut out);
        out
    }

    #[test]
    fn staging_copies_a_fresh_tree_and_leaves_the_source_unchanged() {
        let temp = TempDir::new("stage");
        let source = temp.0.join("source");
        fs::create_dir_all(source.join("src/nested")).unwrap();
        fs::write(source.join("build.zig"), "pub fn build() void {}").unwrap();
        fs::write(source.join("src/nested/file.zig"), "const x = 1;").unwrap();
        symlink(Path::new("src/nested/file.zig"), &source.join("alias"))
            .unwrap();
        let before = snapshot(&source);

        let destination = temp.0.join("out/ghostty-source");
        stage_source(&source, &destination).unwrap();
        assert_eq!(snapshot(&destination), before);

        // Zig creates mutable package directories beside build.zig; the
        // next build must start from a clean copy without them.
        fs::create_dir_all(destination.join("zig-pkg/dep")).unwrap();
        fs::write(destination.join("src/nested/file.zig"), "changed").unwrap();
        stage_source(&source, &destination).unwrap();
        assert_eq!(snapshot(&destination), before);
        assert_eq!(snapshot(&source), before);
        assert_eq!(
            fs::read_link(destination.join("alias")).unwrap(),
            Path::new("src/nested/file.zig")
        );
    }

    #[test]
    fn staging_rejects_a_missing_or_unprepared_source() {
        let temp = TempDir::new("reject");
        let destination = temp.0.join("out");
        assert!(stage_source(&temp.0.join("missing"), &destination).is_err());
        let empty = temp.0.join("empty");
        fs::create_dir_all(&empty).unwrap();
        let error = stage_source(&empty, &destination).unwrap_err();
        assert!(error.to_string().contains("ghostty:prepare"), "{error}");
    }

    #[test]
    fn layout_keeps_caches_outside_the_refreshed_copy() {
        let layout = Layout::new(Path::new("/out"));
        assert!(!layout.local_cache.starts_with(&layout.source));
        assert!(!layout.install.starts_with(&layout.source));
        assert_eq!(layout.source.parent(), Some(Path::new("/out")));
    }
}
