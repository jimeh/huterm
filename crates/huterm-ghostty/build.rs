//! Builds the static libghostty-vt archive from the verified Ghostty source.
//!
//! Mise prepares and hash-checks the source tree, and Cargo configuration
//! points `GHOSTTY_SOURCE_DIR` at it. Zig writes mutable `zig-pkg`
//! directories beside `build.zig`, so every build runs from a fresh private
//! copy under `OUT_DIR` and never touches the verified tree. Zig's caches sit
//! beside that copy, or in Zig's global cache, so they survive a refresh.
//!
//! With `HUTERM_GHOSTTY_ARTIFACT_CACHE` set, a build first looks there for an
//! archive built from the same inputs and links it instead of running Zig.
//! Otherwise it builds from source and stores the result for the next build.
//! Each stored archive carries a fingerprint of everything that shapes it:
//! the source manifest, the Zig version and arguments, the target, and the
//! host libc or SDK. A different fingerprint means a rebuild, never reuse.
//! CI sets the variable; release builds and local builds leave it unset.
//!
//! The staging, option parsing, and cache handling below are plain functions
//! with unit tests; `scripts/ghostty-build.test.ts` compiles this file with
//! `rustc --test`.

use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const SOURCE_VAR: &str = "GHOSTTY_SOURCE_DIR";
const OPTIMIZE_VAR: &str = "HUTERM_GHOSTTY_OPTIMIZE";
const CPU_VAR: &str = "HUTERM_GHOSTTY_CPU";
const ZIG_VAR: &str = "ZIG";
const DEPLOYMENT_VAR: &str = "MACOSX_DEPLOYMENT_TARGET";
const CACHE_VAR: &str = "HUTERM_GHOSTTY_ARTIFACT_CACHE";

/// Changes whenever the cached layout or the fingerprint format changes.
const CACHE_FORMAT: &str = "huterm-ghostty prebuilt 1";
const ARCHIVE: &str = "libghostty-vt.a";

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

/// Reduces the native target Zig reports, such as
/// `aarch64-linux.7.0.14...7.0.14-gnu.2.35`, to its architecture, OS, and
/// libc: kernel and macOS versions do not shape the archive, but glibc does.
fn host_target(native: &str) -> Option<String> {
    let mut parts = native.split('-');
    let arch = parts.next().filter(|arch| !arch.is_empty())?;
    let os = parts
        .next()?
        .split('.')
        .next()
        .filter(|os| !os.is_empty())?;
    let abi = parts.next().filter(|abi| !abi.is_empty())?;
    parts.next().is_none().then(|| format!("{arch}-{os}-{abi}"))
}

/// The `.target` field of `zig env`'s ZON output.
fn zig_env_target(output: &str) -> Option<&str> {
    let start = output.find(".target = \"")? + ".target = \"".len();
    let length = output[start..].find('"')?;
    Some(&output[start..start + length])
}

/// Everything that shapes the archive, as one comparable text. Paths are
/// placeholders because each build installs into its own `OUT_DIR`.
fn fingerprint(
    target: &str,
    options: &Options,
    zig_version: &str,
    host: &str,
    sdk: Option<&str>,
    source_manifest: &str,
) -> String {
    let arguments = options
        .zig_arguments(Path::new("<install>"), Path::new("<cache>"))
        .iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "{CACHE_FORMAT}\ntarget={target}\nzig={zig_version}\nhost={host}\n\
         sdk={}\narguments={arguments}\nsource=\n{source_manifest}",
        sdk.unwrap_or("none")
    )
}

/// The cache entry for one target and build mode; a different fingerprint
/// in the same slot is replaced, never reused.
fn cache_slot(cache: &Path, target: &str, options: &Options) -> PathBuf {
    cache.join(format!(
        "{target}-{}-{}",
        options.optimize.as_str(),
        options.cpu
    ))
}

/// Installs the slot's archive and headers when its fingerprint matches;
/// returns false, leaving `install` alone, when it does not.
fn reuse_cached(
    slot: &Path,
    fingerprint: &str,
    install: &Path,
) -> Result<bool, BuildError> {
    let recorded = match fs::read_to_string(slot.join("fingerprint")) {
        Ok(recorded) => recorded,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(false);
        }
        // Like a failed store, an unreadable entry only costs a rebuild.
        Err(error) => {
            println!(
                "cargo:warning=could not read the cached libghostty-vt in {}: \
                 {error}",
                slot.display()
            );
            return Ok(false);
        }
    };
    let archive = slot.join("lib").join(ARCHIVE);
    // An empty archive would otherwise surface only as a link error.
    let usable = fs::metadata(&archive)
        .is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0);
    if recorded != fingerprint || !usable || !slot.join("include").is_dir() {
        return Ok(false);
    }
    replace_dir(install)?;
    let library = install.join("lib");
    fs::create_dir_all(&library).map_err(|error| {
        BuildError::Io(format!("creating {}", library.display()), error)
    })?;
    fs::copy(&archive, library.join(ARCHIVE)).map_err(|error| {
        BuildError::Io(format!("copying {}", archive.display()), error)
    })?;
    copy_tree(&slot.join("include"), &install.join("include"))?;
    Ok(true)
}

/// Stores a fresh build in `slot`. It fills a private sibling first and
/// writes the fingerprint last, then swaps it in, so an interrupted store
/// never leaves a matching fingerprint beside a partial archive.
fn store_cached(
    slot: &Path,
    fingerprint: &str,
    install: &Path,
) -> Result<(), BuildError> {
    let name = slot.file_name().map_or_else(
        || "slot".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let staging =
        slot.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    replace_dir(&staging)?;
    let library = staging.join("lib");
    fs::create_dir_all(&library).map_err(|error| {
        BuildError::Io(format!("creating {}", library.display()), error)
    })?;
    let archive = install.join("lib").join(ARCHIVE);
    fs::copy(&archive, library.join(ARCHIVE)).map_err(|error| {
        BuildError::Io(format!("copying {}", archive.display()), error)
    })?;
    copy_tree(&install.join("include"), &staging.join("include"))?;
    fs::write(staging.join("fingerprint"), fingerprint).map_err(|error| {
        BuildError::Io(format!("writing {}", staging.display()), error)
    })?;
    replace_dir(slot)?;
    fs::remove_dir(slot).map_err(|error| {
        BuildError::Io(format!("removing {}", slot.display()), error)
    })?;
    fs::rename(&staging, slot).map_err(|error| {
        BuildError::Io(format!("storing {}", slot.display()), error)
    })
}

/// Removes `path` if it exists and leaves an empty directory there.
fn replace_dir(path: &Path) -> Result<(), BuildError> {
    match fs::remove_dir_all(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(BuildError::Io(
                format!("removing {}", path.display()),
                error,
            ));
        }
    }
    fs::create_dir_all(path).map_err(|error| {
        BuildError::Io(format!("creating {}", path.display()), error)
    })
}

/// Output of a helper command, or `None` when it cannot run or fails.
fn command_output(program: &OsString, arguments: &[&str]) -> Option<String> {
    let output = Command::new(program).args(arguments).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// The fingerprint for this build, or `None` with a warning when an input
/// cannot be read, in which case the build neither reuses nor stores.
fn current_fingerprint(
    zig: &OsString,
    target: &str,
    options: &Options,
    source_manifest: &Path,
) -> Option<String> {
    let manifest = fs::read_to_string(source_manifest).ok();
    let version = command_output(zig, &["version"]);
    let host = command_output(zig, &["env"])
        .as_deref()
        .and_then(zig_env_target)
        .and_then(host_target);
    let sdk = if target.ends_with("-apple-darwin") {
        command_output(
            &OsString::from("xcrun"),
            &["--sdk", "macosx", "--show-sdk-version"],
        )
        .map(Some)
    } else {
        Some(None)
    };
    if let (Some(manifest), Some(version), Some(host), Some(sdk)) =
        (manifest, version, host, sdk)
    {
        Some(fingerprint(
            target,
            options,
            &version,
            &host,
            sdk.as_deref(),
            &manifest,
        ))
    } else {
        println!(
            "cargo:warning={CACHE_VAR} ignored: could not read the Zig \
             version, host target, SDK, or source manifest"
        );
        None
    }
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

/// Pauses between attempts to fetch Ghostty's Zig packages. Upstream package
/// hosts have returned 503 during builds, and in CI one such response failed
/// every compiling job of a run.
const FETCH_RETRY_DELAYS: [Duration; 2] =
    [Duration::from_secs(5), Duration::from_secs(20)];

/// The Zig build command, run from the private copy. With `fetch`, it only
/// fetches the packages that exact build needs into Zig's global cache.
///
/// Commit hooks export repository paths that would point Ghostty's version
/// probe at Huterm, so every inherited `GIT_*` variable is removed, and the
/// ceiling stops discovery above the private copy.
fn zig_command(
    zig: &OsString,
    layout: &Layout,
    options: &Options,
    fetch: bool,
    inherited: impl IntoIterator<Item = (OsString, OsString)>,
) -> Command {
    let mut arguments =
        options.zig_arguments(&layout.install, &layout.local_cache);
    if fetch {
        // `zig build` stays first; `--fetch` applies to the same options.
        arguments.insert(1, "--fetch".into());
    }
    let mut command = Command::new(zig);
    command.current_dir(&layout.source).args(arguments);
    for (key, _) in inherited {
        if key.to_str().is_some_and(|key| key.starts_with("GIT_")) {
            command.env_remove(key);
        }
    }
    if let Some(parent) = layout.source.parent() {
        command.env("GIT_CEILING_DIRECTORIES", parent);
    }
    command
}

/// Runs `action` until it succeeds, pausing for each delay in turn, and
/// returns the last error once the delays run out. Only failures Zig reports
/// are retried; an I/O error such as a missing `zig` cannot clear itself.
fn retry<T>(
    delays: &[Duration],
    mut pause: impl FnMut(Duration),
    mut action: impl FnMut() -> Result<T, BuildError>,
) -> Result<T, BuildError> {
    let mut delays = delays.iter();
    loop {
        match action() {
            Ok(value) => return Ok(value),
            Err(error @ BuildError::Zig(_)) => match delays.next() {
                Some(delay) => {
                    println!(
                        "cargo:warning={error}; retrying in {}s",
                        delay.as_secs()
                    );
                    pause(*delay);
                }
                None => return Err(error),
            },
            Err(error) => return Err(error),
        }
    }
}

fn run_zig(
    zig: &OsString,
    layout: &Layout,
    options: &Options,
    fetch: bool,
) -> Result<(), BuildError> {
    let status = zig_command(zig, layout, options, fetch, std::env::vars_os())
        .status()
        .map_err(|error| {
            BuildError::Io(
                format!(
                    "running {}; install the pinned Zig with `mise install`",
                    zig.to_string_lossy()
                ),
                error,
            )
        })?;
    if !status.success() {
        let step = if fetch {
            "zig build --fetch"
        } else {
            "zig build"
        };
        return Err(BuildError::Zig(format!(
            "{step} failed with {status} in {}",
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
    let cache = optional(CACHE_VAR)
        .filter(|cache| !cache.is_empty())
        .and_then(|cache| {
            let fingerprint =
                current_fingerprint(&zig, &target, &options, &manifest)?;
            Some((
                cache_slot(Path::new(&cache), &target, &options),
                fingerprint,
            ))
        });
    if let Some((slot, fingerprint)) = &cache
        && reuse_cached(slot, fingerprint, &layout.install)?
    {
        println!(
            "cargo:warning=reused the cached libghostty-vt in {}",
            slot.display()
        );
    } else {
        stage_source(&source, &layout.source)?;
        // Fetch first, with retries, so a transient package-host failure
        // cannot fail the build itself.
        retry(&FETCH_RETRY_DELAYS, std::thread::sleep, || {
            run_zig(&zig, &layout, &options, true)
        })?;
        run_zig(&zig, &layout, &options, false)?;
        if !layout.install.join("lib").join(ARCHIVE).is_file() {
            return Err(BuildError::Zig(format!(
                "zig build did not produce {}",
                layout.install.join("lib").join(ARCHIVE).display()
            )));
        }
        // The copy exists only for Zig; a rebuild stages a fresh one.
        // Dropping it keeps each build directory about 650 MB smaller.
        if let Err(error) = fs::remove_dir_all(&layout.source) {
            println!(
                "cargo:warning=could not remove {}: {error}",
                layout.source.display()
            );
        }
        // A failed store only costs the next build a rebuild.
        if let Some((slot, fingerprint)) = &cache
            && let Err(error) = store_cached(slot, fingerprint, &layout.install)
        {
            println!("cargo:warning=could not cache libghostty-vt: {error}");
        }
    }

    let library = layout.install.join("lib");
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
    fn zig_runs_in_the_copy_without_inherited_git_state() {
        let parsed = options(
            Some("ReleaseFast"),
            Some("baseline"),
            "aarch64-apple-darwin",
            None,
        )
        .unwrap();
        let layout = Layout::new(Path::new("/out"));
        let inherited = [
            ("GIT_DIR", "/repo/.git"),
            ("GIT_INDEX_FILE", "/repo/.git/index"),
            ("HOME", "/home/user"),
        ]
        .map(|(key, value)| (OsString::from(key), OsString::from(value)));
        let command = zig_command(
            &OsString::from("zig"),
            &layout,
            &parsed,
            false,
            inherited,
        );
        assert_eq!(command.get_current_dir(), Some(layout.source.as_path()));
        let mut environment: Vec<_> = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_str().unwrap(),
                    value.map(|value| value.to_str().unwrap()),
                )
            })
            .collect();
        environment.sort_unstable();
        assert_eq!(
            environment,
            [
                ("GIT_CEILING_DIRECTORIES", Some("/out")),
                ("GIT_DIR", None),
                ("GIT_INDEX_FILE", None),
            ]
        );
    }

    #[test]
    fn host_targets_keep_libc_and_drop_os_versions() {
        assert_eq!(
            host_target("aarch64-linux.7.0.14...7.0.14-gnu.2.35").as_deref(),
            Some("aarch64-linux-gnu.2.35")
        );
        assert_eq!(
            host_target("aarch64-macos.27.0...27.0-none").as_deref(),
            Some("aarch64-macos-none")
        );
        for malformed in
            ["", "aarch64", "aarch64-linux", "a-b-c-d", "-linux-gnu"]
        {
            assert_eq!(host_target(malformed), None, "{malformed:?}");
        }
        let env = ".{\n    .version = \"0.16.0\",\n    .target = \"x86_64-linux.6.8.0...6.8.0-gnu.2.39\",\n}";
        assert_eq!(
            zig_env_target(env),
            Some("x86_64-linux.6.8.0...6.8.0-gnu.2.39")
        );
        assert_eq!(zig_env_target(".{ .version = \"0.16.0\" }"), None);
    }

    #[test]
    fn fingerprints_change_with_every_input_that_shapes_the_archive() {
        let target = "x86_64-unknown-linux-gnu";
        let fast = options(Some("ReleaseFast"), Some("baseline"), target, None)
            .unwrap();
        let base = fingerprint(
            target,
            &fast,
            "0.16.0",
            "x86_64-linux-gnu.2.35",
            None,
            "{pin}",
        );
        let safe = options(Some("ReleaseSafe"), Some("baseline"), target, None)
            .unwrap();
        let variants = [
            fingerprint(
                "aarch64-unknown-linux-gnu",
                &fast,
                "0.16.0",
                "x86_64-linux-gnu.2.35",
                None,
                "{pin}",
            ),
            fingerprint(
                target,
                &safe,
                "0.16.0",
                "x86_64-linux-gnu.2.35",
                None,
                "{pin}",
            ),
            fingerprint(
                target,
                &fast,
                "0.16.1",
                "x86_64-linux-gnu.2.35",
                None,
                "{pin}",
            ),
            fingerprint(
                target,
                &fast,
                "0.16.0",
                "x86_64-linux-gnu.2.39",
                None,
                "{pin}",
            ),
            fingerprint(
                target,
                &fast,
                "0.16.0",
                "x86_64-linux-gnu.2.35",
                Some("26.0"),
                "{pin}",
            ),
            fingerprint(
                target,
                &fast,
                "0.16.0",
                "x86_64-linux-gnu.2.35",
                None,
                "{new pin}",
            ),
        ];
        for variant in &variants {
            assert_ne!(variant, &base);
        }
        assert_eq!(
            base,
            fingerprint(
                target,
                &fast,
                "0.16.0",
                "x86_64-linux-gnu.2.35",
                None,
                "{pin}"
            )
        );
        assert!(base.starts_with(CACHE_FORMAT));
        assert!(!base.contains("/out"), "{base}");
        assert_eq!(
            cache_slot(Path::new("/cache"), target, &fast),
            Path::new("/cache/x86_64-unknown-linux-gnu-ReleaseFast-baseline")
        );
    }

    fn fake_install(root: &Path, archive: &str) -> PathBuf {
        let install = root.join("install");
        fs::create_dir_all(install.join("lib")).unwrap();
        fs::create_dir_all(install.join("include/ghostty")).unwrap();
        fs::write(install.join("lib").join(ARCHIVE), archive).unwrap();
        fs::write(install.join("lib/libghostty-vt.dylib"), "shared").unwrap();
        fs::write(install.join("include/ghostty/vt.h"), "header").unwrap();
        install
    }

    #[test]
    fn stored_archives_are_reused_only_for_the_same_fingerprint() {
        let temp = TempDir::new("cache-reuse");
        let slot = temp.0.join("cache/slot");
        let built = fake_install(&temp.0.join("built"), "archive-one");
        store_cached(&slot, "print-one", &built).unwrap();
        // Only what Huterm links is kept: the static archive and headers.
        assert!(!slot.join("lib/libghostty-vt.dylib").exists());

        let reused = temp.0.join("reused");
        fs::create_dir_all(&reused).unwrap();
        fs::write(reused.join("stale"), "from an earlier build").unwrap();
        assert!(reuse_cached(&slot, "print-one", &reused).unwrap());
        assert_eq!(
            fs::read_to_string(reused.join("lib").join(ARCHIVE)).unwrap(),
            "archive-one"
        );
        assert_eq!(
            fs::read_to_string(reused.join("include/ghostty/vt.h")).unwrap(),
            "header"
        );
        assert!(!reused.join("stale").exists());

        let untouched = temp.0.join("untouched");
        fs::create_dir_all(&untouched).unwrap();
        assert!(!reuse_cached(&slot, "print-two", &untouched).unwrap());
        assert!(
            !reuse_cached(
                &temp.0.join("cache/absent"),
                "print-one",
                &untouched
            )
            .unwrap()
        );
        assert!(fs::read_dir(&untouched).unwrap().next().is_none());

        // A slot whose archive is empty or missing is rebuilt, not linked.
        fs::write(slot.join("lib").join(ARCHIVE), "").unwrap();
        assert!(!reuse_cached(&slot, "print-one", &untouched).unwrap());
        fs::remove_file(slot.join("lib").join(ARCHIVE)).unwrap();
        assert!(!reuse_cached(&slot, "print-one", &untouched).unwrap());

        // So is one whose fingerprint cannot be read.
        fs::remove_file(slot.join("fingerprint")).unwrap();
        fs::create_dir(slot.join("fingerprint")).unwrap();
        assert!(!reuse_cached(&slot, "print-one", &untouched).unwrap());
        assert!(fs::read_dir(&untouched).unwrap().next().is_none());
    }

    #[test]
    fn storing_replaces_the_previous_slot_without_leftovers() {
        let temp = TempDir::new("cache-replace");
        let slot = temp.0.join("cache/slot");
        store_cached(
            &slot,
            "print-one",
            &fake_install(&temp.0.join("first"), "archive-one"),
        )
        .unwrap();
        fs::write(slot.join("leftover"), "old").unwrap();
        store_cached(
            &slot,
            "print-two",
            &fake_install(&temp.0.join("second"), "archive-two"),
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(slot.join("fingerprint")).unwrap(),
            "print-two"
        );
        assert_eq!(
            fs::read_to_string(slot.join("lib").join(ARCHIVE)).unwrap(),
            "archive-two"
        );
        assert!(!slot.join("leftover").exists());
        let siblings: Vec<_> = fs::read_dir(temp.0.join("cache"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(siblings, ["slot"]);
    }

    #[test]
    fn layout_keeps_caches_outside_the_refreshed_copy() {
        let layout = Layout::new(Path::new("/out"));
        assert!(!layout.local_cache.starts_with(&layout.source));
        assert!(!layout.install.starts_with(&layout.source));
        assert_eq!(layout.source.parent(), Some(Path::new("/out")));
    }

    #[test]
    fn fetch_runs_the_build_arguments_with_fetch_after_build() {
        let parsed = options(
            Some("ReleaseFast"),
            Some("baseline"),
            "aarch64-apple-darwin",
            None,
        )
        .unwrap();
        let layout = Layout::new(Path::new("/out"));
        let arguments = |fetch| -> Vec<OsString> {
            zig_command(&OsString::from("zig"), &layout, &parsed, fetch, [])
                .get_args()
                .map(OsString::from)
                .collect()
        };
        let mut expected = arguments(false);
        assert_eq!(expected[0], "build");
        expected.insert(1, "--fetch".into());
        assert_eq!(arguments(true), expected);
    }

    #[test]
    fn retry_pauses_between_failures_and_returns_the_last_error() {
        let delays = [Duration::from_secs(5), Duration::from_secs(20)];
        let mut pauses = Vec::new();
        let mut calls = 0;
        let result = retry(
            &delays,
            |delay| pauses.push(delay),
            || {
                calls += 1;
                if calls < 3 {
                    Err(BuildError::Zig(format!("attempt {calls}")))
                } else {
                    Ok(calls)
                }
            },
        );
        assert_eq!(result.unwrap(), 3);
        assert_eq!(pauses, delays);

        let mut pauses = Vec::new();
        let mut calls = 0;
        let result: Result<(), _> = retry(
            &delays,
            |delay| pauses.push(delay),
            || {
                calls += 1;
                Err(BuildError::Zig(format!("attempt {calls}")))
            },
        );
        assert_eq!(result.unwrap_err().to_string(), "attempt 3");
        assert_eq!(pauses, delays);

        let mut pauses = Vec::new();
        let result: Result<(), _> = retry(
            &delays,
            |delay| pauses.push(delay),
            || {
                Err(BuildError::Io(
                    "running zig".into(),
                    io::Error::from(io::ErrorKind::NotFound),
                ))
            },
        );
        assert!(matches!(result, Err(BuildError::Io(..))));
        assert!(pauses.is_empty());
    }
}
