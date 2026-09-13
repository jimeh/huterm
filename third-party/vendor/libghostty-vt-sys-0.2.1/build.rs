use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Pinned ghostty commit. Update this to pull a newer version.
const GHOSTTY_REPO: &str = "https://github.com/ghostty-org/ghostty.git";
const GHOSTTY_COMMIT: &str = "22d13172cde98a0a4dda05d3d6a3fcb0dd8ed018";

/// File name of the static archive on Windows. Ghostty installs it under this
/// name for every Windows ABI so it does not collide with `ghostty-vt.lib`,
/// the import library for `ghostty-vt.dll`. Validation and link emission must
/// agree on it, or the build silently links the import library instead.
const WINDOWS_STATIC_LIB_FILE: &str = "ghostty-vt-static.lib";

#[derive(Clone, Copy)]
enum LinkMode {
    Dynamic,
    Static,
}

impl LinkMode {
    fn current() -> Self {
        if cfg!(feature = "link-dynamic") {
            Self::Dynamic
        } else {
            Self::Static
        }
    }

    fn artifact_kind(self) -> &'static str {
        match self {
            Self::Dynamic => "shared library",
            Self::Static => "static library",
        }
    }

    fn matches_library(self, target: &str, file_name: &str) -> bool {
        match self {
            Self::Dynamic => {
                if target.contains("darwin") {
                    file_name.starts_with("libghostty-vt") && file_name.ends_with(".dylib")
                } else if target.contains("windows") {
                    file_name == "ghostty-vt.lib"
                        || file_name == "ghostty-vt.dll"
                        || file_name == "libghostty-vt.dll.lib"
                        || file_name == "libghostty-vt.dll.a"
                } else {
                    file_name == "libghostty-vt.so" || file_name.starts_with("libghostty-vt.so.")
                }
            }
            Self::Static => {
                if target.contains("windows") {
                    file_name == WINDOWS_STATIC_LIB_FILE
                } else {
                    file_name == "libghostty-vt.a"
                }
            }
        }
    }

    #[cfg(feature = "pkg-config")]
    fn pkg_config_name(self) -> &'static str {
        match self {
            Self::Dynamic => "libghostty-vt",
            Self::Static => "libghostty-vt-static",
        }
    }
}

fn main() {
    // docs.rs has no Zig toolchain. The checked-in bindings in src/bindings.rs
    // are enough for generating documentation, so skip the entire native
    // build when running under docs.rs.
    if env::var("DOCS_RS").is_ok() {
        return;
    }

    // Miri cannot load or call the native Ghostty library. The Miri suite stays
    // within Rust-owned seams, so there is nothing to build or link for it.
    if env::var("CARGO_CFG_MIRI").is_ok() {
        return;
    }

    let link_mode = LinkMode::current();
    // Cargo always sets TARGET for build scripts, so read it once here and
    // hand it to whichever path runs rather than re-reading it per call site.
    let target = env::var("TARGET").expect("TARGET must be set");

    println!("cargo:rerun-if-env-changed=LIBGHOSTTY_VT_SYS_CPU");
    println!("cargo:rerun-if-env-changed=LIBGHOSTTY_VT_SYS_OPTIMIZE");
    println!("cargo:rerun-if-env-changed=GHOSTTY_SOURCE_DIR");
    println!("cargo:rerun-if-env-changed=GHOSTTY_ZIG_SYSTEM_DIR");
    println!("cargo:rerun-if-env-changed=TARGET");
    println!("cargo:rerun-if-env-changed=HOST");
    println!("cargo:rerun-if-env-changed=DEBUG");
    println!("cargo:rerun-if-env-changed=OPT_LEVEL");
    println!("cargo:rerun-if-changed=build.rs");

    // An explicit source override should stay authoritative even when the
    // pkg-config feature is enabled, so local Ghostty checkouts remain easy to
    // test against.
    if env::var_os("GHOSTTY_SOURCE_DIR").is_some() {
        build_vendored(link_mode, &target);
        return;
    }

    // When the pkg-config feature is enabled, prefer an installed library over
    // fetching Ghostty. libghostty is pre-1.0, so this crate intentionally does
    // not promise compatibility with every installed C API revision.
    #[cfg(feature = "pkg-config")]
    if try_pkg_config(link_mode, &target) {
        return;
    }

    build_vendored(link_mode, &target);
}

/// Build libghostty-vt from source via zig. The zig build itself generates
/// shared and static artifacts plus pkg-config files in `share/pkgconfig/`.
fn build_vendored(link_mode: LinkMode, target: &str) {
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR must be set"));
    let host = env::var("HOST").expect("HOST must be set");

    // Locate ghostty source: env override > fetch into OUT_DIR.
    let ghostty_source_dir = match env::var("GHOSTTY_SOURCE_DIR") {
        Ok(dir) => {
            let p = PathBuf::from(dir);
            assert!(
                p.join("build.zig").exists(),
                "GHOSTTY_SOURCE_DIR does not contain build.zig: {}",
                p.display()
            );
            p
        }
        Err(_) => fetch_ghostty(&out_dir),
    };

    let ghostty_dir = stage_build_source(&ghostty_source_dir, &out_dir);

    // Build libghostty-vt via zig.
    let install_prefix = out_dir.join("ghostty-install");
    let zig_cache_dir = out_dir.join("zig-cache");
    let zig_global_cache_dir = out_dir.join("zig-global-cache");

    let optimize = zig_optimize_mode();
    let cpu = env::var("LIBGHOSTTY_VT_SYS_CPU").unwrap_or_else(|_| "baseline".to_owned());
    assert!(
        !cpu.is_empty(),
        "LIBGHOSTTY_VT_SYS_CPU must not be empty when set"
    );

    // iOS builds go through ghostty's emit-xcframework path instead of a flat
    // `-Dtarget=<ios> --sysroot=<sdk>` invocation. The flat build breaks down
    // for iOS: a generic target baseline can't compile simdutf's always_inline
    // NEON intrinsics, and `--sysroot` applies globally so it leaks into the
    // native codegen tools ghostty runs mid-build. The xcframework path runs
    // host-native and configures each Apple platform itself, so we build that
    // and pull out the library we need afterwards.
    let ios_platform = ios_xcframework_platform(target);
    if ios_platform.is_some() {
        // Ghostty only emits the xcframework when zig itself runs on macOS
        // (it shells out to xcodebuild). Without this check a Linux cross
        // build would silently produce a host-native library and then fail on
        // a confusing missing-xcframework assertion after the full build.
        assert!(
            host.contains("apple-darwin"),
            "building for {target} requires a macOS host with Xcode and the iOS SDK \
             (ghostty's emit-xcframework path runs host-native); host is {host}"
        );
        // The xcframework contains only static archives, and the flat layout
        // below is populated from one of them. Fail up front instead of
        // letting the shared-library search fail with a misleading message.
        assert!(
            matches!(link_mode, LinkMode::Static),
            "building for {target} supports static linking only; \
             disable the link-dynamic feature"
        );
    }

    let mut build = Command::new("zig");
    build
        .arg("build")
        .arg("-Demit-lib-vt=true")
        .arg(format!("-Doptimize={optimize}"))
        // Cargo artifacts may run on older CPUs than the build host. Without
        // an explicit CPU model, Zig may emit host-specific instructions that
        // make distributed binaries fail with an illegal instruction. Users
        // building for a known machine can explicitly request `native` or a
        // named Zig CPU model through LIBGHOSTTY_VT_SYS_CPU.
        //
        // For iOS builds this only affects the host-native flat artifacts:
        // ghostty resolves its own per-platform targets for the xcframework
        // slices, so -Dcpu does not leak into them.
        .arg(format!("-Dcpu={cpu}"))
        .arg(if ios_platform.is_some() {
            "-Demit-xcframework=true"
        } else {
            "-Demit-xcframework=false"
        })
        .arg("-Dapp-runtime=none")
        .arg("--prefix")
        .arg(&install_prefix)
        .arg("--cache-dir")
        .arg(&zig_cache_dir)
        .current_dir(&ghostty_dir);
    isolate_git_discovery(&mut build, &out_dir);

    // Package managers can provide Ghostty's Zig package cache ahead of time
    // and ask Zig to resolve packages from that immutable store path instead
    // of fetching during this Cargo build script.
    if let Ok(dir) = env::var("GHOSTTY_ZIG_SYSTEM_DIR") {
        assert!(
            !dir.is_empty(),
            "GHOSTTY_ZIG_SYSTEM_DIR must not be empty when set"
        );
        let zig_system_dir = PathBuf::from(dir);
        assert!(
            zig_system_dir.exists(),
            "GHOSTTY_ZIG_SYSTEM_DIR does not exist: {}",
            zig_system_dir.display()
        );
        build
            .arg("--system")
            .arg(&zig_system_dir)
            .arg("--global-cache-dir")
            .arg(&zig_global_cache_dir);
    }

    // Pass -Dtarget only for non-iOS cross targets; native builds let zig
    // auto-detect the host. iOS builds run host-native inside the xcframework
    // emit, so they must not pass -Dtarget or a global --sysroot.
    if target != host && ios_platform.is_none() {
        let zig_target = zig_target(target);
        build.arg(format!("-Dtarget={zig_target}"));
    }

    run(build, "zig build");

    // The emit also installs host-native flat artifacts; replace them with the
    // iOS library so the link emission below picks up the right arch.
    if let Some(platform) = ios_platform {
        extract_xcframework_lib(&install_prefix, platform);
    }

    let lib_dir = install_prefix.join("lib");
    let include_dir = install_prefix.join("include");
    let search_dirs = library_search_dirs(target, &install_prefix);
    if ios_platform.is_none() {
        warn_unused_xcframework(&lib_dir);
    }

    let has_requested_library = search_dirs.iter().any(|dir| {
        std::fs::read_dir(dir)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", dir.display()))
            .any(|entry| {
                let entry = entry.unwrap_or_else(|error| {
                    panic!("failed to read entry from {}: {error}", dir.display())
                });
                let file_name = entry.file_name();
                let Some(file_name) = file_name.to_str() else {
                    return false;
                };

                link_mode.matches_library(target, file_name)
            })
    });
    assert!(
        has_requested_library,
        "expected libghostty-vt {} in one of {:?}",
        link_mode.artifact_kind(),
        search_dirs
    );
    assert!(
        include_dir.join("ghostty").join("vt.h").exists(),
        "expected header at {}",
        include_dir.join("ghostty").join("vt.h").display()
    );

    for dir in &search_dirs {
        println!("cargo:rustc-link-search=native={}", dir.display());
    }
    match link_mode {
        LinkMode::Dynamic => println!("cargo:rustc-link-lib=dylib=ghostty-vt"),
        LinkMode::Static => emit_static_link_lib(target),
    }
    emit_include_metadata(&[include_dir]);
}

/// Emit the link directive for the static archive.
///
/// Zig names static archives `<name>.lib` on every Windows ABI, so neither
/// name rustc derives from a plain `static=ghostty-vt` finds the archive
/// there: MSVC resolves it to `ghostty-vt.lib`, the DLL import library, which
/// links but leaves a load-time dependency on `ghostty-vt.dll`; the GNU
/// targets look for `libghostty-vt.a`, which no Windows build produces, and
/// fail outright. Link the exact file name Ghostty installs instead, which
/// `+verbatim` passes through to rustc's archive lookup untouched.
fn emit_static_link_lib(target: &str) {
    if target.contains("windows") {
        println!("cargo:rustc-link-lib=static:+verbatim={WINDOWS_STATIC_LIB_FILE}");
    } else {
        println!("cargo:rustc-link-lib=static=ghostty-vt");
    }
}

/// Copy the source into disposable build storage because Zig may write package
/// metadata next to build.zig. Recreate it on every build-script invocation so
/// no generated file can survive into the next native build.
fn stage_build_source(source: &Path, out_dir: &Path) -> PathBuf {
    let source = fs::canonicalize(source)
        .unwrap_or_else(|error| panic!("failed to resolve {}: {error}", source.display()));
    let out_dir = fs::canonicalize(out_dir)
        .unwrap_or_else(|error| panic!("failed to resolve {}: {error}", out_dir.display()));
    let destination = out_dir.join("ghostty-build-source");
    let resolved_destination = match fs::canonicalize(&destination) {
        Ok(destination) => destination,
        Err(error) if error.kind() == ErrorKind::NotFound => destination.clone(),
        Err(error) => panic!("failed to resolve {}: {error}", destination.display()),
    };
    assert!(
        !destination.starts_with(&source)
            && !source.starts_with(&destination)
            && !resolved_destination.starts_with(&source)
            && !source.starts_with(&resolved_destination),
        "Ghostty source and build destination must not overlap: source {}, destination {}",
        source.display(),
        resolved_destination.display()
    );
    remove_existing(&destination);
    copy_source_tree(&source, &destination);
    destination
}

fn remove_existing(path: &Path) {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            fs::remove_dir_all(path)
                .unwrap_or_else(|error| panic!("failed to remove {}: {error}", path.display()));
        }
        Ok(_) => {
            fs::remove_file(path)
                .unwrap_or_else(|error| panic!("failed to remove {}: {error}", path.display()));
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => panic!("failed to inspect {}: {error}", path.display()),
    }
}

fn copy_source_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination)
        .unwrap_or_else(|error| panic!("failed to create {}: {error}", destination.display()));
    let entries = fs::read_dir(source)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", source.display()));
    for entry in entries {
        let entry = entry
            .unwrap_or_else(|error| panic!("failed to read entry from {}: {error}", source.display()));
        let name = entry.file_name();
        if name == OsStr::new(".git") {
            continue;
        }
        let source_entry = entry.path();
        let destination_entry = destination.join(&name);
        let metadata = fs::symlink_metadata(&source_entry)
            .unwrap_or_else(|error| panic!("failed to inspect {}: {error}", source_entry.display()));
        if metadata.is_dir() {
            copy_source_tree(&source_entry, &destination_entry);
            continue;
        }
        if metadata.file_type().is_symlink() {
            let target = fs::metadata(&source_entry).unwrap_or_else(|error| {
                panic!("failed to inspect symlink target {}: {error}", source_entry.display())
            });
            assert!(
                target.is_file(),
                "source symlink must resolve to a file: {}",
                source_entry.display()
            );
        } else if !metadata.is_file() {
            panic!("unsupported source entry: {}", source_entry.display());
        }
        fs::copy(&source_entry, &destination_entry).unwrap_or_else(|error| {
            panic!(
                "failed to copy {} to {}: {error}",
                source_entry.display(),
                destination_entry.display()
            )
        });
    }
}

fn isolate_git_discovery(command: &mut Command, out_dir: &Path) {
    let ceiling = fs::canonicalize(out_dir)
        .unwrap_or_else(|error| panic!("failed to resolve {}: {error}", out_dir.display()));
    command
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env("GIT_CEILING_DIRECTORIES", ceiling);
}

fn warn_unused_xcframework(lib_dir: &Path) {
    let xcframework = lib_dir.join("ghostty-vt.xcframework");
    if xcframework.exists() {
        println!(
            "cargo:warning=unused libghostty-vt XCFramework emitted at {}; Cargo links the dylib or archive directly",
            xcframework.display()
        );
    }
}

#[cfg(feature = "pkg-config")]
fn try_pkg_config(link_mode: LinkMode, target: &str) -> bool {
    let mut config = pkg_config::Config::new();
    let lib = match link_mode {
        LinkMode::Dynamic => config.probe(link_mode.pkg_config_name()),
        LinkMode::Static => config
            .statik(true)
            .cargo_metadata(false)
            .probe(link_mode.pkg_config_name()),
    };
    let lib = match lib {
        Ok(lib) => lib,
        Err(_) => return false,
    };

    if let LinkMode::Static = link_mode {
        emit_static_pkg_config_metadata(&lib, target);
    }
    emit_include_metadata(&lib.include_paths);
    true
}

#[cfg(feature = "pkg-config")]
fn emit_static_pkg_config_metadata(lib: &pkg_config::Library, target: &str) {
    for path in &lib.link_paths {
        println!("cargo:rustc-link-search=native={}", path.display());
    }
    for path in &lib.link_files {
        if let Some(parent) = path.parent() {
            println!("cargo:rustc-link-search=native={}", parent.display());
        }
    }
    for path in &lib.framework_paths {
        println!("cargo:rustc-link-search=framework={}", path.display());
    }
    for framework in &lib.frameworks {
        println!("cargo:rustc-link-lib=framework={framework}");
    }

    emit_static_link_lib(target);
    for library in &lib.libs {
        if library != "ghostty-vt" {
            println!("cargo:rustc-link-lib={library}");
        }
    }
    for args in &lib.ld_args {
        if !args.is_empty() {
            println!("cargo:rustc-link-arg=-Wl,{}", args.join(","));
        }
    }
}

fn emit_include_metadata(include_paths: &[PathBuf]) {
    if include_paths.is_empty() {
        return;
    }

    let joined = env::join_paths(include_paths)
        .unwrap_or_else(|error| panic!("failed to join include paths for cargo metadata: {error}"));
    println!("cargo:include={}", joined.to_string_lossy());
}

/// Decide which Zig `OptimizeMode` to pass to `zig build`.
///
/// The `LIBGHOSTTY_VT_SYS_OPTIMIZE` environment variable overrides this unconditionally; accepted
/// values are the four Zig `OptimizeMode` names (`Debug`, `ReleaseSafe`, `ReleaseFast`,
/// `ReleaseSmall`).
///
/// Defaults to `ReleaseFast` for optimized builds. If `DEBUG` is `true` (as cargo sets for the
/// `dev` profile), `Debug` mode is used. Otherwise, if `OPT_LEVEL` is `s` or `z`, `ReleaseSmall`
/// is used.
fn zig_optimize_mode() -> &'static str {
    if let Ok(override_mode) = env::var("LIBGHOSTTY_VT_SYS_OPTIMIZE") {
        return match override_mode.as_str() {
            "Debug" => "Debug",
            "ReleaseSafe" => "ReleaseSafe",
            "ReleaseFast" => "ReleaseFast",
            "ReleaseSmall" => "ReleaseSmall",
            other => panic!(
                "LIBGHOSTTY_VT_SYS_OPTIMIZE must be one of Debug, ReleaseSafe, ReleaseFast, ReleaseSmall (got '{other}')"
            ),
        };
    }

    if env::var("DEBUG").as_deref() == Ok("true") {
        return "Debug";
    }

    match env::var("OPT_LEVEL").as_deref() {
        Ok("s") | Ok("z") => "ReleaseSmall",
        _ => "ReleaseFast",
    }
}

/// Clone ghostty at the pinned commit into OUT_DIR/ghostty-src.
/// Reuses an existing clone if the commit matches.
fn fetch_ghostty(out_dir: &Path) -> PathBuf {
    let src_dir = out_dir.join("ghostty-src");
    let stamp = src_dir.join(".ghostty-commit");

    // Skip fetch if we already have the right commit.
    if stamp.exists()
        && let Ok(existing) = std::fs::read_to_string(&stamp)
        && existing.trim() == GHOSTTY_COMMIT
    {
        return src_dir;
    }

    // Clean and clone fresh.
    if src_dir.exists() {
        std::fs::remove_dir_all(&src_dir)
            .unwrap_or_else(|e| panic!("failed to remove {}: {e}", src_dir.display()));
    }

    eprintln!("Fetching ghostty {GHOSTTY_COMMIT} ...");

    let mut clone = Command::new("git");
    clone
        .arg("clone")
        .arg("--filter=blob:none")
        .arg("--no-checkout")
        .arg(GHOSTTY_REPO)
        .arg(&src_dir);
    run(clone, "git clone ghostty");

    let mut checkout = Command::new("git");
    checkout
        .arg("checkout")
        .arg(GHOSTTY_COMMIT)
        .current_dir(&src_dir);
    run(checkout, "git checkout ghostty commit");

    std::fs::write(&stamp, GHOSTTY_COMMIT).unwrap_or_else(|e| panic!("failed to write stamp: {e}"));

    src_dir
}

fn run(mut command: Command, context: &str) {
    let status = command
        .status()
        .unwrap_or_else(|error| panic!("failed to execute {context}: {error}"));
    assert!(status.success(), "{context} failed with status {status}");
}

/// Returns directories to search for the built library artifact.
/// On Windows, Zig may place the DLL in `bin/` and the import lib in `lib/`,
/// so both are included.
fn library_search_dirs(target: &str, install_prefix: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![install_prefix.join("lib")];
    if target.contains("windows") {
        dirs.push(install_prefix.join("bin"));
    }
    dirs
}

/// The platform directory inside `ghostty-vt.xcframework` for an iOS Rust
/// target, or `None` for targets that build directly via `-Dtarget`. Ghostty
/// emits an arm64-only simulator library (what the simulator runs on Apple
/// silicon), so x86_64-apple-ios is not supported here.
fn ios_xcframework_platform(target: &str) -> Option<&'static str> {
    match target {
        "aarch64-apple-ios" => Some("ios-arm64"),
        "aarch64-apple-ios-sim" => Some("ios-arm64-simulator"),
        _ => None,
    }
}

/// Copy an xcframework platform's static lib + headers into the flat
/// `<prefix>/lib/libghostty-vt.a` and `<prefix>/include` layout the link
/// emission expects, replacing the host-native artifacts the emit installed.
fn extract_xcframework_lib(install_prefix: &Path, platform: &str) {
    let platform_dir = install_prefix
        .join("lib")
        .join("ghostty-vt.xcframework")
        .join(platform);
    // Ghostty emits iOS xcframework slices only when it detects the iOS SDK,
    // silently skipping them otherwise, so a missing directory here almost
    // always means the SDK is absent rather than a build failure.
    assert!(
        platform_dir.is_dir(),
        "expected xcframework platform dir {platform} at {}; \
         ghostty emits iOS slices only when the iOS SDK is detected, \
         so install it via Xcode (Settings > Components)",
        platform_dir.display()
    );
    // ghostty names the Apple static libraries `libghostty-vt-fat.a`.
    let src_lib = platform_dir.join("libghostty-vt-fat.a");
    assert!(
        src_lib.exists(),
        "expected static lib at {}",
        src_lib.display()
    );

    let lib_dir = install_prefix.join("lib");
    let dest_lib = lib_dir.join("libghostty-vt.a");
    std::fs::copy(&src_lib, &dest_lib).unwrap_or_else(|error| {
        panic!(
            "failed to copy {} -> {}: {error}",
            src_lib.display(),
            dest_lib.display()
        )
    });

    // Headers are arch-independent, but prefer the platform's own copy.
    let headers = platform_dir.join("Headers");
    if headers.is_dir() {
        copy_dir_all(&headers, &install_prefix.join("include"));
    }
}

/// Recursively copy a directory tree (merging into `dst` if it exists).
fn copy_dir_all(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst)
        .unwrap_or_else(|error| panic!("failed to create {}: {error}", dst.display()));
    let entries = std::fs::read_dir(src)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", src.display()));
    for entry in entries {
        let entry = entry
            .unwrap_or_else(|error| panic!("failed to read entry in {}: {error}", src.display()));
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let file_type = entry
            .file_type()
            .unwrap_or_else(|error| panic!("failed to stat {}: {error}", from.display()));
        if file_type.is_dir() {
            copy_dir_all(&from, &to);
        } else {
            std::fs::copy(&from, &to)
                .unwrap_or_else(|error| panic!("failed to copy {}: {error}", from.display()));
        }
    }
}

fn zig_target(target: &str) -> String {
    let value = match target {
        "x86_64-unknown-linux-gnu" => "x86_64-linux-gnu",
        "x86_64-unknown-linux-musl" => "x86_64-linux-musl",
        "aarch64-unknown-linux-gnu" => "aarch64-linux-gnu",
        "aarch64-unknown-linux-musl" => "aarch64-linux-musl",
        "aarch64-apple-darwin" => "aarch64-macos-none",
        "x86_64-apple-darwin" => "x86_64-macos-none",
        "x86_64-pc-windows-gnu" => "x86_64-windows-gnu",
        "aarch64-pc-windows-gnullvm" => "aarch64-windows-gnu",
        "x86_64-pc-windows-msvc" => "x86_64-windows-msvc",
        "aarch64-pc-windows-msvc" => "aarch64-windows-msvc",
        "aarch64-linux-android" => "aarch64-linux-android",
        "x86_64-linux-android" => "x86_64-linux-android",
        other => panic!("unsupported Rust target for vendored build: {other}"),
    };
    value.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Stdio;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new(name: &str) -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock must follow the Unix epoch")
                .as_nanos();
            let path = env::temp_dir().join(format!(
                "libghostty-vt-sys-{name}-{}-{unique}",
                std::process::id()
            ));
            fs::create_dir(&path).expect("create test directory");
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn staging_copies_source_and_removes_previous_build_outputs() {
        let root = TestDirectory::new("staging");
        let source = root.0.join("source");
        let out_dir = root.0.join("out");
        fs::create_dir_all(source.join("src/nested")).expect("create source directories");
        fs::create_dir(&out_dir).expect("create output directory");
        fs::write(source.join("build.zig"), "authoritative build\n").expect("write build file");
        fs::write(source.join("src/nested/value.txt"), "nested source\n")
            .expect("write nested file");

        let staged = stage_build_source(&source, &out_dir);
        assert_eq!(
            fs::read_to_string(staged.join("build.zig")).expect("read staged build file"),
            "authoritative build\n"
        );
        assert_eq!(
            fs::read_to_string(staged.join("src/nested/value.txt"))
                .expect("read staged nested file"),
            "nested source\n"
        );

        fs::write(staged.join("build.zig"), "mutated build\n").expect("mutate staged file");
        fs::create_dir(staged.join("zig-pkg")).expect("create generated package directory");
        fs::write(staged.join("zig-pkg/generated"), "generated\n")
            .expect("write generated package file");

        let rebuilt = stage_build_source(&source, &out_dir);
        assert_eq!(rebuilt, staged);
        assert!(!rebuilt.join("zig-pkg").exists());
        assert_eq!(
            fs::read_to_string(rebuilt.join("build.zig")).expect("read rebuilt file"),
            "authoritative build\n"
        );
        assert_eq!(
            fs::read_to_string(source.join("build.zig")).expect("read original build file"),
            "authoritative build\n"
        );
        assert!(!source.join("zig-pkg").exists());
    }

    #[cfg(unix)]
    #[test]
    fn staging_dereferences_source_file_symlinks() {
        use std::os::unix::fs::symlink;

        let root = TestDirectory::new("symlink");
        let source = root.0.join("source");
        let out_dir = root.0.join("out");
        fs::create_dir(&source).expect("create source directory");
        fs::create_dir(&out_dir).expect("create output directory");
        fs::write(source.join("AGENTS.md"), "source instructions\n")
            .expect("write symlink target");
        symlink("AGENTS.md", source.join("CLAUDE.md")).expect("create source symlink");

        let staged = stage_build_source(&source, &out_dir);
        assert_eq!(
            fs::read_to_string(staged.join("CLAUDE.md")).expect("read staged symlink content"),
            "source instructions\n"
        );
        assert!(!fs::symlink_metadata(staged.join("CLAUDE.md"))
            .expect("inspect staged file")
            .file_type()
            .is_symlink());
        assert!(fs::symlink_metadata(source.join("CLAUDE.md"))
            .expect("inspect original symlink")
            .file_type()
            .is_symlink());
    }

    #[test]
    fn staging_rejects_a_destination_inside_the_source_before_copying() {
        let root = TestDirectory::new("destination-inside-source");
        let source = root.0.join("source");
        fs::create_dir(&source).expect("create source directory");
        fs::write(source.join("build.zig"), "authoritative build\n").expect("write build file");

        let result = std::panic::catch_unwind(|| stage_build_source(&source, &source));
        assert!(result.is_err(), "overlapping paths must be rejected");
        assert_eq!(
            fs::read_to_string(source.join("build.zig")).expect("read preserved source file"),
            "authoritative build\n"
        );
        assert!(!source.join("ghostty-build-source").exists());
    }

    #[test]
    fn staging_rejects_a_source_inside_the_destination_before_deleting() {
        let root = TestDirectory::new("source-inside-destination");
        let out_dir = root.0.join("out");
        let destination = out_dir.join("ghostty-build-source");
        let source = destination.join("source");
        fs::create_dir_all(&source).expect("create nested source directory");
        fs::write(source.join("build.zig"), "authoritative build\n").expect("write build file");

        let result = std::panic::catch_unwind(|| stage_build_source(&source, &out_dir));
        assert!(result.is_err(), "overlapping paths must be rejected");
        assert_eq!(
            fs::read_to_string(source.join("build.zig")).expect("read preserved source file"),
            "authoritative build\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn staging_rejects_a_destination_symlink_to_the_source() {
        use std::os::unix::fs::symlink;

        let root = TestDirectory::new("destination-symlink");
        let source = root.0.join("source");
        let out_dir = root.0.join("out");
        fs::create_dir(&source).expect("create source directory");
        fs::create_dir(&out_dir).expect("create output directory");
        fs::write(source.join("build.zig"), "authoritative build\n").expect("write build file");
        symlink(&source, out_dir.join("ghostty-build-source"))
            .expect("create destination symlink");

        let result = std::panic::catch_unwind(|| stage_build_source(&source, &out_dir));
        assert!(result.is_err(), "aliased overlapping paths must be rejected");
        assert_eq!(
            fs::read_to_string(source.join("build.zig")).expect("read preserved source file"),
            "authoritative build\n"
        );
        assert!(
            fs::symlink_metadata(out_dir.join("ghostty-build-source"))
                .expect("inspect preserved destination symlink")
                .file_type()
                .is_symlink()
        );
    }

    #[cfg(unix)]
    #[test]
    fn staging_rejects_a_symlink_entry_inside_the_source() {
        use std::os::unix::fs::symlink;

        let root = TestDirectory::new("destination-symlink-entry");
        let source = root.0.join("source");
        let external = root.0.join("external");
        fs::create_dir(&source).expect("create source directory");
        fs::create_dir(&external).expect("create external directory");
        fs::write(source.join("build.zig"), "authoritative build\n").expect("write build file");
        fs::write(external.join("sentinel"), "external data\n").expect("write external file");
        let destination = source.join("ghostty-build-source");
        symlink(&external, &destination).expect("create destination symlink");

        let result = std::panic::catch_unwind(|| stage_build_source(&source, &source));
        assert!(result.is_err(), "lexically overlapping paths must be rejected");
        assert_eq!(
            fs::read_to_string(external.join("sentinel")).expect("read preserved external file"),
            "external data\n"
        );
        assert!(
            fs::symlink_metadata(&destination)
                .expect("inspect preserved destination symlink")
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn zig_command_cannot_discover_a_repository_above_out_dir() {
        fn git_command(directory: &Path) -> Command {
            let mut command = Command::new("git");
            command
                .arg("-c")
                .arg("core.hooksPath=/dev/null")
                .current_dir(directory)
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            for (name, _) in env::vars_os() {
                if name.to_string_lossy().starts_with("GIT_") {
                    command.env_remove(name);
                }
            }
            command
        }

        let root = TestDirectory::new("git-ceiling");
        let mut initialize = git_command(&root.0);
        initialize.args(["init", "--quiet"]);
        assert!(initialize.status().expect("run git init").success());

        let out_dir = root.0.join("out");
        let build_source = out_dir.join("ghostty-build-source");
        fs::create_dir_all(&build_source).expect("create staged source directory");
        let mut unrestricted = git_command(&build_source);
        unrestricted.args(["rev-parse", "--show-toplevel"]);
        assert!(
            unrestricted
                .status()
                .expect("run unrestricted git discovery")
                .success()
        );

        let mut isolated = git_command(&build_source);
        isolated.args(["rev-parse", "--show-toplevel"]);
        isolated
            .env("GIT_DIR", root.0.join(".git"))
            .env("GIT_WORK_TREE", &root.0);
        isolate_git_discovery(&mut isolated, &out_dir);
        assert!(!isolated.status().expect("run isolated git discovery").success());

        let mut hostile_control = git_command(&build_source);
        hostile_control
            .args(["rev-parse", "--show-toplevel"])
            .env("GIT_DIR", root.0.join(".git"))
            .env("GIT_WORK_TREE", &root.0)
            .env(
                "GIT_CEILING_DIRECTORIES",
                fs::canonicalize(&out_dir).expect("resolve output directory"),
            );
        assert!(
            hostile_control
                .status()
                .expect("run hostile git discovery control")
                .success(),
            "Git overrides must bypass the ceiling in the control command"
        );
    }
}
